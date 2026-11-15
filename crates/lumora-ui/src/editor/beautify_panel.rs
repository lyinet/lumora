//! The editor's beautify panel: a glass column on the right of the canvas
//! with the enable switch, background presets / custom colour / image,
//! padding, corners, shadow, window frame, aspect ratio and auto-balance.
//!
//! The panel only reports [`PanelEvent`]s; the editor folds them into its
//! [`BeautifyStyle`] (via [`BeautifyStyle::apply`]), re-lays the live preview
//! and writes the style back to Settings. [`BeautifyPanel::sync`] mirrors a
//! style into the controls without emitting anything.

use std::rc::Rc;

use lumora_core::beautify::{
    Adjust, AspectRatio, Background, BeautifyStyle, Preset, Rgba, WindowChrome, CORNER_RADIUS_RANGE, PADDING_RANGE,
    SHADOW_OPACITY_RANGE, SHADOW_RADIUS_RANGE,
};
use lumora_core::geometry::{Rect, Size};
use lumora_core::i18n::{tr, Msg};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAutoresizingMaskOptions, NSButton, NSColor, NSColorWell,
    NSControl, NSControlSize, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSScrollView,
    NSSegmentSwitchTracking, NSSegmentedControl, NSSlider, NSSwitch, NSTextAlignment, NSTextField,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSArray, NSString};
use objc2_quartz_core::{CAGradientLayer, CALayer};

use super::views::FlippedView;
use crate::tokens::{color, motion};
use crate::util::{cf_as_object, cg_color, cg_rect, rgb8_of, without_actions, ActionTarget};

/// Panel width in points.
pub const WIDTH: f64 = 248.0;
const PAD: f64 = 14.0;
const SWATCH: f64 = 22.0;
const ROW_GAP: f64 = 10.0;

/// What the panel asks the editor for.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelEvent {
    Adjust(Adjust),
    /// "Image…": run an open panel for a wallpaper.
    ChooseImage,
}

pub type PanelHandler = Rc<dyn Fn(PanelEvent)>;

/// Turns a control's action into a panel event.
type EventMapper = Box<dyn Fn(&AnyObject) -> Option<PanelEvent>>;

/// A beautify colour as a `CGColor`.
pub fn cg_rgba(c: Rgba) -> CFRetained<CGColor> {
    let [r, g, b, a] = c.components();
    CGColor::new_srgb(r, g, b, a)
}

/// Set a two-stop gradient on `layer`.
pub fn set_gradient(layer: &CAGradientLayer, from: Rgba, to: Rgba, angle_deg: f32, size: Size) {
    let (a, b) = (cg_rgba(from), cg_rgba(to));
    let colors: Retained<NSArray> = NSArray::from_slice(&[cf_as_object(&*a), cf_as_object(&*b)]);
    // SAFETY: an array of CGColors is what `colors` expects.
    unsafe { layer.setColors(Some(&colors)) };
    let (s, e) = lumora_core::beautify::gradient_unit_points(size, angle_deg);
    layer.setStartPoint(CGPoint::new(s.x, s.y));
    layer.setEndPoint(CGPoint::new(e.x, e.y));
}

struct SliderRow {
    slider: Retained<NSSlider>,
    value: Retained<NSTextField>,
}

pub struct BeautifyPanel {
    pub view: Retained<NSVisualEffectView>,
    enable: Retained<NSSwitch>,
    presets: Vec<(Preset, Retained<NSButton>)>,
    well: Retained<NSColorWell>,
    image: Retained<NSButton>,
    padding: SliderRow,
    corner: SliderRow,
    shadow_on: Retained<NSButton>,
    shadow_radius: SliderRow,
    shadow_opacity: SliderRow,
    chrome: Retained<NSSegmentedControl>,
    aspect: Retained<NSSegmentedControl>,
    balance: Retained<NSButton>,
    /// Everything that greys out while beautify is off.
    dependents: Vec<Retained<NSControl>>,
    _targets: Vec<Retained<ActionTarget>>,
}

impl std::fmt::Debug for BeautifyPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BeautifyPanel").finish_non_exhaustive()
    }
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font = if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
    l.setFont(Some(&font));
    if !bold {
        l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    l
}

fn checkbox(mtm: MainThreadMarker, title: &str, target: &ActionTarget) -> Retained<NSButton> {
    // SAFETY: the target is kept alive by the panel.
    let b = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.setControlSize(NSControlSize::Small);
    b.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    b
}

fn segmented(mtm: MainThreadMarker, labels: &[&str], target: &ActionTarget) -> Retained<NSSegmentedControl> {
    let labels: Vec<Retained<NSString>> = labels.iter().map(|s| NSString::from_str(s)).collect();
    // SAFETY: the target is kept alive by the panel.
    let seg = unsafe {
        NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
            &NSArray::from_retained_slice(&labels),
            NSSegmentSwitchTracking::SelectOne,
            Some(target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    seg.setControlSize(NSControlSize::Small);
    seg.setFont(Some(&NSFont::systemFontOfSize(10.5)));
    seg
}

fn seg_index(sender: &AnyObject) -> Option<usize> {
    sender.downcast_ref::<NSSegmentedControl>().map(|s| s.selectedSegment().max(0) as usize)
}

fn slider_value(sender: &AnyObject) -> Option<f64> {
    sender.downcast_ref::<NSSlider>().map(|s| s.doubleValue())
}

fn button_on(sender: &AnyObject) -> bool {
    sender.downcast_ref::<NSButton>().is_some_and(|b| b.state() == NSControlStateValueOn)
}

fn state(on: bool) -> isize {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

impl BeautifyPanel {
    /// Build the panel at the right edge of a `width × height` area (the
    /// canvas, below the toolbar). Starts hidden.
    pub fn new(mtm: MainThreadMarker, width: f64, height: f64, on: PanelHandler) -> Self {
        let frame = Rect::new(width - WIDTH, 0.0, WIDTH, height);
        let view = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), cg_rect(frame));
        view.setMaterial(NSVisualEffectMaterial::Sidebar);
        view.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setWantsLayer(true);
        // Hairline on the leading edge.
        if let Some(l) = view.layer() {
            let line = CALayer::new();
            line.setBackgroundColor(Some(&cg_color(color::HAIRLINE)));
            line.setFrame(cg_rect(Rect::new(0.0, 0.0, 0.5, 10_000.0)));
            l.addSublayer(&line);
        }

        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), cg_rect(Rect::new(0.0, 0.0, WIDTH, height)));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.addSubview(&scroll);

        let doc = FlippedView::new(mtm, cg_rect(Rect::new(0.0, 0.0, WIDTH, 600.0)));
        let inner = WIDTH - 2.0 * PAD;
        let mut targets: Vec<Retained<ActionTarget>> = Vec::new();
        let mut dependents: Vec<Retained<NSControl>> = Vec::new();
        let mut y = PAD;

        let target_for = |f: EventMapper, targets: &mut Vec<Retained<ActionTarget>>| {
            let h = on.clone();
            let t = ActionTarget::new(mtm, move |sender| {
                if let Some(e) = f(sender) {
                    h(e);
                }
            });
            targets.push(t.clone());
            t
        };

        // Title + enable switch.
        let title = label(mtm, tr(Msg::Beautify), 13.0, true);
        title.setFrame(cg_rect(Rect::new(PAD, y + 2.0, inner - 50.0, 18.0)));
        doc.addSubview(&title);
        let t = target_for(
            Box::new(|s| {
                s.downcast_ref::<NSSwitch>()
                    .map(|sw| PanelEvent::Adjust(Adjust::Enabled(sw.state() == NSControlStateValueOn)))
            }),
            &mut targets,
        );
        let enable =
            NSSwitch::initWithFrame(NSSwitch::alloc(mtm), cg_rect(Rect::new(WIDTH - PAD - 38.0, y, 38.0, 22.0)));
        enable.setControlSize(NSControlSize::Small);
        // SAFETY: the target is kept alive by the panel.
        unsafe {
            enable.setTarget(Some(&t));
            enable.setAction(Some(ActionTarget::action()));
        }
        doc.addSubview(&enable);
        y += 22.0 + ROW_GAP + 4.0;

        // Background presets.
        let bg = label(mtm, tr(Msg::BeautifyBackground), 11.0, false);
        bg.setFrame(cg_rect(Rect::new(PAD, y, inner, 15.0)));
        doc.addSubview(&bg);
        y += 15.0 + 6.0;
        let gap = (inner - SWATCH * Preset::ALL.len() as f64) / (Preset::ALL.len() as f64 - 1.0);
        let mut presets = Vec::new();
        for (i, p) in Preset::ALL.iter().copied().enumerate() {
            let t = target_for(
                Box::new(move |_| Some(PanelEvent::Adjust(Adjust::Background(p.background())))),
                &mut targets,
            );
            // SAFETY: the target is kept alive by the panel.
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::new(), Some(&t), Some(ActionTarget::action()), mtm)
            };
            b.setBordered(false);
            b.setToolTip(Some(&NSString::from_str(tr(p.name_key()))));
            b.setFrame(cg_rect(Rect::new(PAD + i as f64 * (SWATCH + gap), y, SWATCH, SWATCH)));
            b.setWantsLayer(true);
            if let Some(l) = b.layer() {
                l.setCornerRadius(SWATCH / 2.0);
                l.setBorderWidth(0.5);
                l.setBorderColor(Some(&cg_color(color::HAIRLINE.with_alpha(0.35))));
                let g = CAGradientLayer::new();
                g.setFrame(cg_rect(Rect::new(1.5, 1.5, SWATCH - 3.0, SWATCH - 3.0)));
                g.setCornerRadius((SWATCH - 3.0) / 2.0);
                let (from, to, angle) = p.stops();
                set_gradient(&g, from, to, angle, Size::new(SWATCH, SWATCH));
                l.addSublayer(&g);
            }
            doc.addSubview(&b);
            dependents.push(Retained::into_super(b.clone()));
            presets.push((p, b));
        }
        y += SWATCH + ROW_GAP;

        // Custom solid colour + image.
        let t = target_for(
            Box::new(|s| {
                let well = s.downcast_ref::<NSColorWell>()?;
                let c = rgb8_of(&well.color())?;
                Some(PanelEvent::Adjust(Adjust::Background(Background::Solid(Rgba::from(c)))))
            }),
            &mut targets,
        );
        let well = NSColorWell::initWithFrame(NSColorWell::alloc(mtm), cg_rect(Rect::new(PAD, y, 44.0, 24.0)));
        well.setToolTip(Some(&NSString::from_str(tr(Msg::BeautifySolid))));
        well.setContinuous(true);
        // SAFETY: the target is kept alive by the panel.
        unsafe {
            well.setTarget(Some(&t));
            well.setAction(Some(ActionTarget::action()));
        }
        doc.addSubview(&well);
        dependents.push(Retained::into_super(well.clone()));
        let solid = label(mtm, tr(Msg::BeautifySolid), 11.0, false);
        solid.setFrame(cg_rect(Rect::new(PAD + 50.0, y + 4.0, 80.0, 15.0)));
        doc.addSubview(&solid);
        let t = target_for(Box::new(|_| Some(PanelEvent::ChooseImage)), &mut targets);
        // SAFETY: the target is kept alive by the panel.
        let image = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(tr(Msg::BeautifyImage)),
                Some(&t),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        image.setControlSize(NSControlSize::Small);
        image.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        image.sizeToFit();
        let iw = image.frame().size.width.max(70.0);
        image.setFrame(cg_rect(Rect::new(WIDTH - PAD - iw, y, iw, 24.0)));
        doc.addSubview(&image);
        dependents.push(Retained::into_super(image.clone()));
        y += 24.0 + ROW_GAP + 4.0;

        // Sliders.
        let slider_row = |title: Msg,
                          range: (f64, f64),
                          make: fn(f64) -> Adjust,
                          y: &mut f64,
                          targets: &mut Vec<Retained<ActionTarget>>,
                          dependents: &mut Vec<Retained<NSControl>>| {
            let l = label(mtm, tr(title), 11.0, false);
            l.setFrame(cg_rect(Rect::new(PAD, *y, inner - 50.0, 15.0)));
            doc.addSubview(&l);
            let value = label(mtm, "", 11.0, false);
            value.setAlignment(NSTextAlignment::Right);
            value.setFrame(cg_rect(Rect::new(WIDTH - PAD - 50.0, *y, 50.0, 15.0)));
            doc.addSubview(&value);
            *y += 15.0 + 2.0;
            let h = on.clone();
            let t = ActionTarget::new(mtm, move |s| {
                if let Some(v) = slider_value(s) {
                    h(PanelEvent::Adjust(make(v)));
                }
            });
            // SAFETY: the target is kept alive by the panel.
            let slider = unsafe {
                NSSlider::sliderWithValue_minValue_maxValue_target_action(
                    range.0,
                    range.0,
                    range.1,
                    Some(&t),
                    Some(ActionTarget::action()),
                    mtm,
                )
            };
            slider.setControlSize(NSControlSize::Small);
            slider.setContinuous(true);
            slider.setFrame(cg_rect(Rect::new(PAD, *y, inner, 20.0)));
            doc.addSubview(&slider);
            targets.push(t);
            dependents.push(Retained::into_super(slider.clone()));
            *y += 20.0 + ROW_GAP;
            SliderRow { slider, value }
        };
        let padding =
            slider_row(Msg::BeautifyPadding, PADDING_RANGE, Adjust::Padding, &mut y, &mut targets, &mut dependents);
        let corner = slider_row(
            Msg::BeautifyCornerRadius,
            CORNER_RADIUS_RANGE,
            Adjust::CornerRadius,
            &mut y,
            &mut targets,
            &mut dependents,
        );

        let t = target_for(Box::new(|s| Some(PanelEvent::Adjust(Adjust::ShadowEnabled(button_on(s))))), &mut targets);
        let shadow_on = checkbox(mtm, tr(Msg::BeautifyShadow), &t);
        shadow_on.setFrame(cg_rect(Rect::new(PAD - 2.0, y, inner, 18.0)));
        doc.addSubview(&shadow_on);
        dependents.push(Retained::into_super(shadow_on.clone()));
        y += 18.0 + 6.0;
        let shadow_radius = slider_row(
            Msg::BeautifyShadowBlur,
            SHADOW_RADIUS_RANGE,
            Adjust::ShadowRadius,
            &mut y,
            &mut targets,
            &mut dependents,
        );
        let shadow_opacity = slider_row(
            Msg::BeautifyShadowOpacity,
            SHADOW_OPACITY_RANGE,
            Adjust::ShadowOpacity,
            &mut y,
            &mut targets,
            &mut dependents,
        );

        // Window frame.
        let l = label(mtm, tr(Msg::BeautifyWindowChrome), 11.0, false);
        l.setFrame(cg_rect(Rect::new(PAD, y, inner, 15.0)));
        doc.addSubview(&l);
        y += 15.0 + 4.0;
        let t = target_for(
            Box::new(|s| {
                let i = seg_index(s)?.min(WindowChrome::ALL.len() - 1);
                Some(PanelEvent::Adjust(Adjust::Chrome(WindowChrome::ALL[i])))
            }),
            &mut targets,
        );
        let names: Vec<&str> = WindowChrome::ALL.iter().map(|c| tr(c.name_key())).collect();
        let chrome = segmented(mtm, &names, &t);
        for i in 0..names.len() {
            chrome.setWidth_forSegment((inner - 4.0) / names.len() as f64, i as isize);
        }
        chrome.setFrame(cg_rect(Rect::new(PAD, y, inner, 22.0)));
        doc.addSubview(&chrome);
        dependents.push(Retained::into_super(chrome.clone()));
        y += 22.0 + ROW_GAP;

        // Aspect ratio.
        let l = label(mtm, tr(Msg::BeautifyAspectRatio), 11.0, false);
        l.setFrame(cg_rect(Rect::new(PAD, y, inner, 15.0)));
        doc.addSubview(&l);
        y += 15.0 + 4.0;
        let t = target_for(
            Box::new(|s| {
                let i = seg_index(s)?.min(AspectRatio::ALL.len() - 1);
                Some(PanelEvent::Adjust(Adjust::Aspect(AspectRatio::ALL[i])))
            }),
            &mut targets,
        );
        let names: Vec<&str> = AspectRatio::ALL.iter().map(|a| a.label()).collect();
        let aspect = segmented(mtm, &names, &t);
        for i in 0..names.len() {
            aspect.setWidth_forSegment((inner - 7.0) / names.len() as f64, i as isize);
        }
        aspect.setFrame(cg_rect(Rect::new(PAD, y, inner, 22.0)));
        doc.addSubview(&aspect);
        dependents.push(Retained::into_super(aspect.clone()));
        y += 22.0 + ROW_GAP;

        let t = target_for(Box::new(|s| Some(PanelEvent::Adjust(Adjust::AutoBalance(button_on(s))))), &mut targets);
        let balance = checkbox(mtm, tr(Msg::BeautifyAutoBalance), &t);
        balance.setFrame(cg_rect(Rect::new(PAD - 2.0, y, inner, 18.0)));
        doc.addSubview(&balance);
        dependents.push(Retained::into_super(balance.clone()));
        y += 18.0 + PAD;

        doc.setFrame(cg_rect(Rect::new(0.0, 0.0, WIDTH, y)));
        scroll.setDocumentView(Some(&doc));

        view.setHidden(true);
        view.setAlphaValue(0.0);

        BeautifyPanel {
            view,
            enable,
            presets,
            well,
            image,
            padding,
            corner,
            shadow_on,
            shadow_radius,
            shadow_opacity,
            chrome,
            aspect,
            balance,
            dependents,
            _targets: targets,
        }
    }

    pub fn is_open(&self) -> bool {
        !self.view.isHidden()
    }

    /// Fade the panel in or out.
    pub fn set_open(&self, open: bool) {
        if open == self.is_open() {
            return;
        }
        if open {
            self.view.setAlphaValue(0.0);
            self.view.setHidden(false);
            NSAnimationContext::beginGrouping();
            NSAnimationContext::currentContext().setDuration(motion::HOVER_FADE);
            self.view.animator().setAlphaValue(1.0);
            NSAnimationContext::endGrouping();
        } else {
            // Hidden at once so it never takes clicks while fading.
            self.view.setHidden(true);
            self.view.setAlphaValue(0.0);
        }
    }

    /// Whether the colour well currently owns the shared colour panel.
    pub fn well_active(&self) -> bool {
        self.well.isActive()
    }

    pub fn deactivate_well(&self) {
        if self.well.isActive() {
            self.well.deactivate();
        }
    }

    /// Mirror `style` into the controls (emits nothing).
    pub fn sync(&self, style: &BeautifyStyle) {
        self.enable.setState(state(style.enabled));
        let selected = style.background.preset();
        without_actions(|| {
            for (p, b) in &self.presets {
                if let Some(l) = b.layer() {
                    let on = selected == Some(*p);
                    l.setBorderWidth(if on { 2.0 } else { 0.5 });
                    let ring = if on { color::ACCENT } else { color::HAIRLINE.with_alpha(0.35) };
                    l.setBorderColor(Some(&cg_color(ring)));
                }
            }
        });
        if let Background::Solid(c) = &style.background {
            let [r, g, b, a] = c.components();
            self.well.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a));
        }
        self.image.setState(state(matches!(style.background, Background::Image(_))));
        let set = |row: &SliderRow, v: f64, text: String| {
            row.slider.setDoubleValue(v);
            row.value.setStringValue(&NSString::from_str(&text));
        };
        set(&self.padding, style.padding, format!("{:.0}", style.padding));
        set(&self.corner, style.corner_radius, format!("{:.0}", style.corner_radius));
        self.shadow_on.setState(state(style.shadow.enabled));
        set(&self.shadow_radius, style.shadow.radius, format!("{:.0}", style.shadow.radius));
        set(&self.shadow_opacity, style.shadow.opacity, format!("{:.0}%", style.shadow.opacity * 100.0));
        let ci = WindowChrome::ALL.iter().position(|c| *c == style.chrome).unwrap_or(0);
        self.chrome.setSelectedSegment(ci as isize);
        let ai = AspectRatio::ALL.iter().position(|a| *a == style.aspect).unwrap_or(0);
        self.aspect.setSelectedSegment(ai as isize);
        self.balance.setState(state(style.auto_balance));
        for c in &self.dependents {
            c.setEnabled(style.enabled);
        }
        let shadow = style.enabled && style.shadow.enabled;
        self.shadow_radius.slider.setEnabled(shadow);
        self.shadow_opacity.slider.setEnabled(shadow);
    }
}

/// Run an open panel for a background image (PNG, JPEG, HEIC).
pub fn choose_background_image(mtm: MainThreadMarker) -> Option<std::path::PathBuf> {
    use objc2_uniform_type_identifiers::{UTType, UTTypeHEIC, UTTypeJPEG, UTTypePNG};
    let panel = objc2_app_kit::NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(false);
    panel.setCanChooseFiles(true);
    panel.setAllowsMultipleSelection(false);
    // SAFETY: the UTType statics are valid for the process lifetime.
    let types: [&UTType; 3] = unsafe { [UTTypePNG, UTTypeJPEG, UTTypeHEIC] };
    panel.setAllowedContentTypes(&NSArray::from_slice(&types));
    if panel.runModal() != objc2_app_kit::NSModalResponseOK {
        return None;
    }
    panel.URL().and_then(|u| u.path()).map(|p| std::path::PathBuf::from(p.to_string()))
}
