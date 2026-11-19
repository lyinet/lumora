//! The editor toolbar: a titlebar-height strip of `NSVisualEffectView`
//! glass holding the tools (SF Symbols), the palette, stroke widths, the
//! active tool's options and the document actions.
//!
//! The active tool sits on an accent pill that springs between buttons
//! ([`motion::TOOL`]); contextual options cross-fade when the tool changes.
//!
//! When the window is too narrow for everything (the 760 pt minimum is),
//! the trailing items of the leading strip (options, stroke widths, colours,
//! then tools from the end) move into a "…" menu, as decided by
//! [`lumora_core::layout::toolbar_overflow`] on every resize.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lumora_core::annotate::editor::{Tool, ToolOptions};
use lumora_core::annotate::shape::RedactMode;
use lumora_core::annotate::style::{StrokeWidth, Style, OPACITY_MAX, OPACITY_MIN, PALETTE};
use lumora_core::color::Rgb8;
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Msg};
use lumora_core::layout::toolbar_overflow;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSAutoresizingMaskOptions, NSButton, NSColor, NSControlSize,
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSImageSymbolConfiguration, NSMenu, NSMenuItem,
    NSSegmentSwitchTracking, NSSegmentedControl, NSSlider, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSArray, NSString};
use objc2_quartz_core::CALayer;

use super::views::DragTile;
use crate::tokens::{color, motion, radius, size};
use crate::util::{cg_color, cg_rect, cg_rgb8, spring_scalar, symbol, weight, without_actions, ActionTarget};

/// Everything the toolbar can ask for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolbarAction {
    Tool(Tool),
    Color(Rgb8),
    CustomColor,
    Width(StrokeWidth),
    /// Opacity in percent (10–100), from the slider on mouse-up.
    Opacity(u8),
    /// The contextual checkbox (curved / rounded / background).
    Toggle(bool),
    Redact(RedactMode),
    ApplyCrop,
    ResetCrop,
    Undo,
    Redo,
    Copy,
    Save,
    /// Show / hide the beautify panel.
    Beautify,
    /// Open the AI writing menu.
    Ai,
    /// Find and redact sensitive info (⌘⇧R).
    SmartRedact,
    /// The "…" overflow button: pop up the hidden items.
    More,
}

/// One overflowable group of the leading strip.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SlotKind {
    Tool(Tool),
    Swatch(Rgb8),
    Custom,
    Widths,
    Opacity,
    /// The active tool's options (checkbox, redaction mode, crop buttons).
    Options,
}

struct Slot {
    kind: SlotKind,
    views: Vec<Retained<NSView>>,
    /// Width plus the space after it.
    advance: f64,
    x: f64,
}

/// Upcast any view subclass (buttons, segmented controls) to `NSView`.
fn as_view<T: AsRef<NSView> + ?Sized>(v: &T) -> Retained<NSView> {
    use objc2::Message;
    v.as_ref().retain()
}

pub type ToolbarHandler = Rc<dyn Fn(ToolbarAction)>;

fn tool_tip(t: Tool) -> Msg {
    match t {
        Tool::Select => Msg::ToolSelect,
        Tool::Arrow => Msg::ToolArrow,
        Tool::Rect => Msg::ToolRect,
        Tool::Ellipse => Msg::ToolEllipse,
        Tool::Line => Msg::ToolLine,
        Tool::Pen => Msg::ToolPen,
        Tool::Highlighter => Msg::ToolHighlighter,
        Tool::Text => Msg::ToolText,
        Tool::Step => Msg::ToolStep,
        Tool::Redact => Msg::ToolRedact,
        Tool::Spotlight => Msg::ToolSpotlight,
        Tool::Crop => Msg::ToolCrop,
    }
}

/// SF Symbols fall back to similar glyphs on older systems.
fn tool_image(t: Tool) -> Option<Retained<NSImage>> {
    let desc = tr(tool_tip(t));
    symbol(t.symbol(), desc).or_else(|| match t {
        Tool::Redact => symbol("eye.slash", desc),
        Tool::Spotlight => symbol("lightbulb", desc),
        Tool::Highlighter => symbol("pencil.tip", desc),
        _ => symbol("questionmark", desc),
    })
}

pub struct Toolbar {
    pub view: Retained<NSVisualEffectView>,
    tools: Vec<(Tool, Retained<NSButton>)>,
    pill: Retained<CALayer>,
    swatches: Vec<(Rgb8, Retained<NSButton>)>,
    custom: Retained<NSButton>,
    widths: Retained<NSSegmentedControl>,
    opacity: Retained<NSSlider>,
    toggle: Retained<NSButton>,
    redact: Retained<NSSegmentedControl>,
    apply_crop: Retained<NSButton>,
    reset_crop: Retained<NSButton>,
    undo: Retained<NSButton>,
    redo: Retained<NSButton>,
    beautify: Retained<NSButton>,
    ai: Retained<NSButton>,
    smart_redact: Retained<NSButton>,
    more: Retained<NSButton>,
    _drag: Retained<DragTile>,
    _targets: Vec<Retained<ActionTarget>>,
    active: Cell<Option<Tool>>,
    pill_x: Cell<f64>,
    on: ToolbarHandler,
    slots: Vec<Slot>,
    /// How many leading slots are visible (the rest are in the "…" menu).
    visible: Cell<usize>,
    /// Width of the trailing (document actions) cluster incl. margins.
    trailing_width: f64,
    /// Last synced state, for the overflow menu's check marks.
    last: Cell<Option<(Tool, Style, ToolOptions)>>,
    last_has_selection: Cell<bool>,
    menu_targets: RefCell<Vec<Retained<ActionTarget>>>,
}

impl std::fmt::Debug for Toolbar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Toolbar").field("active", &self.active.get()).finish_non_exhaustive()
    }
}

fn glyph_button(
    mtm: MainThreadMarker,
    image: Option<Retained<NSImage>>,
    tip: &str,
    target: &ActionTarget,
) -> Retained<NSButton> {
    // SAFETY: the target outlives the button (kept by the toolbar).
    let b = unsafe {
        match image {
            Some(img) => NSButton::buttonWithImage_target_action(&img, Some(target), Some(ActionTarget::action()), mtm),
            None => NSButton::buttonWithTitle_target_action(
                &NSString::from_str(tip),
                Some(target),
                Some(ActionTarget::action()),
                mtm,
            ),
        }
    };
    b.setBordered(false);
    b.setToolTip(Some(&NSString::from_str(tip)));
    let cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(15.0, weight::MEDIUM);
    b.setSymbolConfiguration(Some(&cfg));
    b.setContentTintColor(Some(&NSColor::labelColor()));
    b
}

fn plain_button(mtm: MainThreadMarker, title: &str, target: &ActionTarget) -> Retained<NSButton> {
    // SAFETY: the target outlives the button.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(ActionTarget::action()),
            mtm,
        )
    };
    b.sizeToFit();
    b
}

fn dot(points: f64) -> Option<Retained<NSImage>> {
    let cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(points, weight::REGULAR);
    symbol("circle.fill", "").and_then(|i| i.imageWithSymbolConfiguration(&cfg))
}

impl Toolbar {
    pub fn new(mtm: MainThreadMarker, width: f64, height: f64, on: ToolbarHandler, drag: Retained<DragTile>) -> Self {
        let frame = Rect::new(0.0, height - size::EDITOR_TOOLBAR_HEIGHT, width, size::EDITOR_TOOLBAR_HEIGHT);
        let view = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), cg_rect(frame));
        view.setMaterial(NSVisualEffectMaterial::Titlebar);
        view.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.setWantsLayer(true);

        let bh = size::EDITOR_TOOL_BUTTON;
        let y = (size::EDITOR_TOOLBAR_HEIGHT - bh) / 2.0;
        let mut targets = Vec::new();
        let mut x = size::EDITOR_TRAFFIC_LIGHTS;

        // Active-tool pill (behind the tool buttons).
        let pill_host = NSView::initWithFrame(
            NSView::alloc(mtm),
            cg_rect(Rect::new(0.0, 0.0, 2000.0, size::EDITOR_TOOLBAR_HEIGHT)),
        );
        pill_host.setWantsLayer(true);
        let pill = CALayer::new();
        pill.setBackgroundColor(Some(&cg_color(color::TOOL_ACTIVE)));
        pill.setCornerRadius(radius::TOOL);
        pill.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        pill.setBounds(cg_rect(Rect::new(0.0, 0.0, bh, bh)));
        pill.setPosition(objc2_core_foundation::CGPoint::new(x + bh / 2.0, size::EDITOR_TOOLBAR_HEIGHT / 2.0));
        if let Some(l) = pill_host.layer() {
            l.addSublayer(&pill);
        }
        view.addSubview(&pill_host);

        let mut tools = Vec::new();
        let mut slots: Vec<Slot> = Vec::new();
        let mut slot = |kind: SlotKind, views: Vec<Retained<NSView>>, x: f64| {
            slots.push(Slot { kind, views, advance: 0.0, x });
        };
        for t in Tool::ALL {
            let h = on.clone();
            let target = ActionTarget::new(mtm, move |_| h(ToolbarAction::Tool(t)));
            let b = glyph_button(mtm, tool_image(t), tr(tool_tip(t)), &target);
            b.setFrame(cg_rect(Rect::new(x, y, bh, bh)));
            view.addSubview(&b);
            slot(SlotKind::Tool(t), vec![as_view(&b)], x);
            tools.push((t, b));
            targets.push(target);
            x += bh + size::EDITOR_TOOL_GAP;
        }
        x += 12.0;

        // Palette.
        let sw = size::EDITOR_SWATCH;
        let mut swatches = Vec::new();
        for c in PALETTE {
            let h = on.clone();
            let target = ActionTarget::new(mtm, move |_| h(ToolbarAction::Color(c)));
            let b = plain_button(mtm, "", &target);
            b.setBordered(false);
            b.setFrame(cg_rect(Rect::new(x, (size::EDITOR_TOOLBAR_HEIGHT - sw) / 2.0, sw, sw)));
            b.setWantsLayer(true);
            if let Some(l) = b.layer() {
                l.setBackgroundColor(Some(&cg_rgb8(c, 1.0)));
                l.setCornerRadius(sw / 2.0);
                l.setBorderColor(Some(&cg_color(color::HAIRLINE.with_alpha(0.35))));
                l.setBorderWidth(0.5);
            }
            view.addSubview(&b);
            slot(SlotKind::Swatch(c), vec![as_view(&b)], x);
            swatches.push((c, b));
            targets.push(target);
            x += sw + 6.0;
        }
        let h = on.clone();
        let target = ActionTarget::new(mtm, move |_| h(ToolbarAction::CustomColor));
        let custom = glyph_button(mtm, symbol("paintpalette", tr(Msg::CustomColor)), tr(Msg::CustomColor), &target);
        custom.setFrame(cg_rect(Rect::new(x, y, bh, bh)));
        view.addSubview(&custom);
        slot(SlotKind::Custom, vec![as_view(&custom)], x);
        targets.push(target);
        x += bh + 10.0;

        // Stroke widths.
        let images: Vec<Retained<NSImage>> = [5.0, 8.0, 12.0].iter().filter_map(|p| dot(*p)).collect();
        let h = on.clone();
        let target = ActionTarget::new(mtm, move |sender: &AnyObject| {
            if let Some(seg) = sender.downcast_ref::<NSSegmentedControl>() {
                let i = seg.selectedSegment().clamp(0, 2) as usize;
                h(ToolbarAction::Width(StrokeWidth::ALL[i]));
            }
        });
        // SAFETY: target kept alive by the toolbar.
        let widths = unsafe {
            if images.len() == 3 {
                NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
                    &NSArray::from_retained_slice(&images),
                    NSSegmentSwitchTracking::SelectOne,
                    Some(&target),
                    Some(ActionTarget::action()),
                    mtm,
                )
            } else {
                let labels: Vec<Retained<NSString>> = ["S", "M", "L"].iter().map(|s| NSString::from_str(s)).collect();
                NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                    &NSArray::from_retained_slice(&labels),
                    NSSegmentSwitchTracking::SelectOne,
                    Some(&target),
                    Some(ActionTarget::action()),
                    mtm,
                )
            }
        };
        for (i, m) in [Msg::StrokeThin, Msg::StrokeMedium, Msg::StrokeThick].iter().enumerate() {
            widths.setToolTip_forSegment(Some(&NSString::from_str(tr(*m))), i as isize);
            widths.setWidth_forSegment(26.0, i as isize);
        }
        widths.sizeToFit();
        let wf = widths.frame();
        widths.setFrame(cg_rect(Rect::new(
            x,
            (size::EDITOR_TOOLBAR_HEIGHT - wf.size.height) / 2.0,
            wf.size.width,
            wf.size.height,
        )));
        view.addSubview(&widths);
        slot(SlotKind::Widths, vec![as_view(&widths)], x);
        targets.push(target);
        x += wf.size.width + 10.0;

        // Opacity. Not continuous: one undo step per adjustment, not one per
        // pixel of slider travel.
        let h = on.clone();
        let target = ActionTarget::new(mtm, move |sender: &AnyObject| {
            if let Some(s) = sender.downcast_ref::<NSSlider>() {
                h(ToolbarAction::Opacity(s.doubleValue().round().clamp(0.0, 100.0) as u8));
            }
        });
        // SAFETY: target kept alive by the toolbar.
        let opacity = unsafe {
            NSSlider::sliderWithValue_minValue_maxValue_target_action(
                f64::from(OPACITY_MAX),
                f64::from(OPACITY_MIN),
                f64::from(OPACITY_MAX),
                Some(&target),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        opacity.setControlSize(NSControlSize::Small);
        opacity.setContinuous(false);
        opacity.setToolTip(Some(&NSString::from_str(&opacity_tip(OPACITY_MAX))));
        let ow = 64.0;
        opacity.setFrame(cg_rect(Rect::new(x, (size::EDITOR_TOOLBAR_HEIGHT - 20.0) / 2.0, ow, 20.0)));
        view.addSubview(&opacity);
        slot(SlotKind::Opacity, vec![as_view(&opacity)], x);
        targets.push(target);
        x += ow + 12.0;

        // Contextual options: one checkbox, the redaction mode, or crop buttons.
        let h = on.clone();
        let target = ActionTarget::new(mtm, move |sender: &AnyObject| {
            let on = sender.downcast_ref::<NSButton>().is_some_and(|b| b.state() == NSControlStateValueOn);
            h(ToolbarAction::Toggle(on));
        });
        // SAFETY: target kept alive by the toolbar.
        let toggle = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str(tr(Msg::OptionBackground)),
                Some(&target),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        toggle.sizeToFit();
        let tf = toggle.frame();
        toggle.setFrame(cg_rect(Rect::new(
            x,
            (size::EDITOR_TOOLBAR_HEIGHT - tf.size.height) / 2.0,
            120.0,
            tf.size.height,
        )));
        view.addSubview(&toggle);
        targets.push(target);

        let h = on.clone();
        let target = ActionTarget::new(mtm, move |sender: &AnyObject| {
            if let Some(seg) = sender.downcast_ref::<NSSegmentedControl>() {
                h(ToolbarAction::Redact(if seg.selectedSegment() == 1 {
                    RedactMode::Blur
                } else {
                    RedactMode::Pixelate
                }));
            }
        });
        let labels: Vec<Retained<NSString>> =
            [Msg::OptionPixelate, Msg::OptionBlur].iter().map(|m| NSString::from_str(tr(*m))).collect();
        // SAFETY: target kept alive by the toolbar.
        let redact = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &NSArray::from_retained_slice(&labels),
                NSSegmentSwitchTracking::SelectOne,
                Some(&target),
                Some(ActionTarget::action()),
                mtm,
            )
        };
        redact.sizeToFit();
        let rf = redact.frame();
        redact.setFrame(cg_rect(Rect::new(
            x,
            (size::EDITOR_TOOLBAR_HEIGHT - rf.size.height) / 2.0,
            rf.size.width,
            rf.size.height,
        )));
        view.addSubview(&redact);
        targets.push(target);

        let h = on.clone();
        let target = ActionTarget::new(mtm, move |_| h(ToolbarAction::ApplyCrop));
        let apply_crop = plain_button(mtm, tr(Msg::ApplyCrop), &target);
        let af = apply_crop.frame();
        apply_crop.setFrame(cg_rect(Rect::new(
            x,
            (size::EDITOR_TOOLBAR_HEIGHT - af.size.height) / 2.0,
            af.size.width,
            af.size.height,
        )));
        view.addSubview(&apply_crop);
        targets.push(target);
        let h = on.clone();
        let target = ActionTarget::new(mtm, move |_| h(ToolbarAction::ResetCrop));
        let reset_crop = plain_button(mtm, tr(Msg::ResetCrop), &target);
        let rcf = reset_crop.frame();
        reset_crop.setFrame(cg_rect(Rect::new(
            x + af.size.width + 6.0,
            (size::EDITOR_TOOLBAR_HEIGHT - rcf.size.height) / 2.0,
            rcf.size.width,
            rcf.size.height,
        )));
        view.addSubview(&reset_crop);
        targets.push(target);
        let options_w = 120.0_f64.max(rf.size.width).max(af.size.width + 6.0 + rcf.size.width);
        slot(
            SlotKind::Options,
            vec![as_view(&toggle), as_view(&redact), as_view(&apply_crop), as_view(&reset_crop)],
            x,
        );
        drop(slot);
        let end = x + options_w;
        for i in 0..slots.len() {
            let next = slots.get(i + 1).map_or(end, |n| n.x);
            slots[i].advance = next - slots[i].x;
        }

        // The "…" overflow button (placed by `fit`).
        let h = on.clone();
        let target = ActionTarget::new(mtm, move |_| h(ToolbarAction::More));
        let more = glyph_button(mtm, symbol("ellipsis.circle", tr(Msg::ToolbarMore)), tr(Msg::ToolbarMore), &target);
        more.setFrame(cg_rect(Rect::new(x, y, bh, bh)));
        more.setHidden(true);
        view.addSubview(&more);
        targets.push(target);

        // Document actions, anchored to the trailing edge.
        let right = Cell::new(width - 12.0);
        drag.setFrame(cg_rect(Rect::new(right.get() - bh, y, bh, bh)));
        drag.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        drag.setToolTip(Some(&NSString::from_str(tr(Msg::DragImage))));
        view.addSubview(&drag);
        right.set(right.get() - bh - 4.0);
        let trailing = |sym: &str, msg: Msg, action: ToolbarAction, targets: &mut Vec<Retained<ActionTarget>>| {
            let h = on.clone();
            let target = ActionTarget::new(mtm, move |_| h(action));
            let b = glyph_button(mtm, symbol(sym, tr(msg)), tr(msg), &target);
            b.setFrame(cg_rect(Rect::new(right.get() - bh, y, bh, bh)));
            b.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            view.addSubview(&b);
            targets.push(target);
            right.set(right.get() - bh - size::EDITOR_TOOL_GAP);
            b
        };
        let _save = trailing("square.and.arrow.down", Msg::SaveImage, ToolbarAction::Save, &mut targets);
        let _copy = trailing("doc.on.doc", Msg::CopyImage, ToolbarAction::Copy, &mut targets);
        right.set(right.get() - 8.0);
        let beautify = trailing("wand.and.stars", Msg::Beautify, ToolbarAction::Beautify, &mut targets);
        if beautify.image().is_none() {
            if let Some(img) = symbol("sparkles", tr(Msg::Beautify)) {
                beautify.setImage(Some(&img));
                beautify.setTitle(&NSString::new());
            }
        }
        beautify.setWantsLayer(true);
        if let Some(l) = beautify.layer() {
            l.setCornerRadius(radius::TOOL);
        }
        let ai = trailing("sparkles", Msg::AiMenuTip, ToolbarAction::Ai, &mut targets);
        if ai.image().is_none() {
            if let Some(img) = symbol("text.bubble", tr(Msg::AiMenu)) {
                ai.setImage(Some(&img));
                ai.setTitle(&NSString::new());
            }
        }
        ai.setEnabled(false);
        ai.setToolTip(Some(&NSString::from_str(tr(Msg::AiMenuDisabledTip))));
        let smart_redact = trailing("eye.slash.circle", Msg::ToolSmartRedact, ToolbarAction::SmartRedact, &mut targets);
        if smart_redact.image().is_none() {
            if let Some(img) = symbol("eye.slash", tr(Msg::ToolSmartRedact)) {
                smart_redact.setImage(Some(&img));
                smart_redact.setTitle(&NSString::new());
            }
        }
        right.set(right.get() - 8.0);
        let redo = trailing("arrow.uturn.forward", Msg::Redo, ToolbarAction::Redo, &mut targets);
        let undo = trailing("arrow.uturn.backward", Msg::Undo, ToolbarAction::Undo, &mut targets);
        let trailing_width = width - right.get();

        let toolbar = Toolbar {
            view,
            tools,
            pill,
            swatches,
            custom,
            widths,
            opacity,
            toggle,
            redact,
            apply_crop,
            reset_crop,
            undo,
            redo,
            beautify,
            ai,
            smart_redact,
            more,
            _drag: drag,
            _targets: targets,
            active: Cell::new(None),
            pill_x: Cell::new(size::EDITOR_TRAFFIC_LIGHTS + bh / 2.0),
            on,
            slots,
            visible: Cell::new(usize::MAX),
            trailing_width,
            last: Cell::new(None),
            last_has_selection: Cell::new(false),
            menu_targets: RefCell::new(Vec::new()),
        };
        toolbar.fit(width);
        toolbar
    }

    /// Re-flow for a new toolbar `width`: move what doesn't fit into "…".
    pub fn fit(&self, width: f64) {
        let start = size::EDITOR_TRAFFIC_LIGHTS;
        let available = width - self.trailing_width - 8.0 - start;
        let advances: Vec<f64> = self.slots.iter().map(|s| s.advance).collect();
        let more_w = size::EDITOR_TOOL_BUTTON + 6.0;
        let n = toolbar_overflow(&advances, available, more_w);
        if self.visible.replace(n) == n {
            return;
        }
        for (i, s) in self.slots.iter().enumerate() {
            if s.kind != SlotKind::Options {
                for v in &s.views {
                    v.setHidden(i >= n);
                }
            }
        }
        let overflow = n < self.slots.len();
        self.more.setHidden(!overflow);
        if overflow {
            let x = self.slots[n].x;
            let f = self.more.frame();
            self.more.setFrame(cg_rect(Rect::new(x, f.origin.y, f.size.width, f.size.height)));
        }
        self.sync_overflowed();
    }

    fn slot_visible(&self, kind: SlotKind) -> bool {
        self.slots.iter().position(|s| s.kind == kind).is_some_and(|i| i < self.visible.get())
    }

    /// Re-apply what depends on which slots are visible: the options and
    /// the pill (hidden when the active tool is in the menu).
    fn sync_overflowed(&self) {
        if let Some((tool, _, options)) = self.last.get() {
            self.sync_options(tool, options);
        }
        let pill_shown = self.active.get().map_or(true, |t| self.slot_visible(SlotKind::Tool(t)));
        without_actions(|| self.pill.setHidden(!pill_shown));
    }

    /// Pop up the overflow menu under "…" with the hidden items.
    pub fn show_overflow_menu(&self) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let (tool, style, options) = match self.last.get() {
            Some(l) => l,
            None => return,
        };
        let n = self.visible.get();
        if n >= self.slots.len() {
            return;
        }
        let mut targets = Vec::new();
        let on = self.on.clone();
        let mut item = |title: &str, image: Option<Retained<NSImage>>, checked: bool, action: ToolbarAction| {
            let h = on.clone();
            let target = ActionTarget::new(mtm, move |_| h(action));
            // SAFETY: the target is kept alive in `menu_targets`.
            let it = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(ActionTarget::action()),
                    &NSString::new(),
                )
            };
            unsafe { it.setTarget(Some(&target)) };
            if let Some(img) = image {
                it.setImage(Some(&img));
            }
            it.setState(if checked { NSControlStateValueOn } else { NSControlStateValueOff });
            targets.push(target);
            it
        };
        let submenu = |title: &str, items: Vec<Retained<NSMenuItem>>| {
            let parent = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    None,
                    &NSString::new(),
                )
            };
            let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
            for i in &items {
                sub.addItem(i);
            }
            parent.setSubmenu(Some(&sub));
            parent
        };
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(tr(Msg::ToolbarMore)));
        menu.setAutoenablesItems(false);
        let hidden: Vec<SlotKind> = self.slots[n..].iter().map(|s| s.kind).collect();
        let crop = tool == Tool::Crop;
        let colorful = (tool.uses_color() || self.last_has_selection.get()) && !crop;
        let mut colors = Vec::new();
        let mut section = false;
        for kind in &hidden {
            match *kind {
                SlotKind::Tool(t) => {
                    menu.addItem(&item(tr(tool_tip(t)), tool_image(t), t == tool, ToolbarAction::Tool(t)));
                    section = true;
                }
                SlotKind::Swatch(c) => {
                    let title = format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b);
                    colors.push(item(
                        &title,
                        Some(crate::icon::swatch_image(c, 14.0)),
                        style.color == c,
                        ToolbarAction::Color(c),
                    ));
                }
                SlotKind::Custom => {
                    colors.push(item(
                        tr(Msg::CustomColor),
                        symbol("paintpalette", tr(Msg::CustomColor)),
                        !PALETTE.contains(&style.color),
                        ToolbarAction::CustomColor,
                    ));
                }
                SlotKind::Widths => {
                    if section {
                        menu.addItem(&NSMenuItem::separatorItem(mtm));
                        section = false;
                    }
                    if !colors.is_empty() {
                        let c = submenu(tr(Msg::ToolbarColor), std::mem::take(&mut colors));
                        c.setEnabled(colorful);
                        menu.addItem(&c);
                    }
                    let msgs = [Msg::StrokeThin, Msg::StrokeMedium, Msg::StrokeThick];
                    let items = StrokeWidth::ALL
                        .iter()
                        .zip(msgs)
                        .map(|(w, m)| item(tr(m), None, style.width == *w, ToolbarAction::Width(*w)))
                        .collect();
                    let w = submenu(tr(Msg::ToolbarStroke), items);
                    w.setEnabled(!crop && tool != Tool::Redact && tool != Tool::Spotlight);
                    menu.addItem(&w);
                }
                SlotKind::Opacity => {
                    let items = OPACITY_PRESETS
                        .iter()
                        .map(|p| item(&format!("{p}%"), None, style.opacity == *p, ToolbarAction::Opacity(*p)))
                        .collect();
                    let o = submenu(tr(Msg::ToolbarOpacity), items);
                    o.setEnabled(colorful);
                    menu.addItem(&o);
                }
                SlotKind::Options => {
                    if section || !colors.is_empty() {
                        menu.addItem(&NSMenuItem::separatorItem(mtm));
                        section = false;
                    }
                    if !colors.is_empty() {
                        let c = submenu(tr(Msg::ToolbarColor), std::mem::take(&mut colors));
                        c.setEnabled(colorful);
                        menu.addItem(&c);
                    }
                    if let Some((m, on)) = toggle_option(tool, options) {
                        menu.addItem(&item(tr(m), None, on, ToolbarAction::Toggle(!on)));
                    }
                    if tool == Tool::Redact {
                        let blur = options.redact == RedactMode::Blur;
                        menu.addItem(&item(
                            tr(Msg::OptionPixelate),
                            None,
                            !blur,
                            ToolbarAction::Redact(RedactMode::Pixelate),
                        ));
                        menu.addItem(&item(tr(Msg::OptionBlur), None, blur, ToolbarAction::Redact(RedactMode::Blur)));
                    }
                    if crop {
                        menu.addItem(&item(tr(Msg::ApplyCrop), None, false, ToolbarAction::ApplyCrop));
                        menu.addItem(&item(tr(Msg::ResetCrop), None, false, ToolbarAction::ResetCrop));
                    }
                }
            }
        }
        if !colors.is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            let c = submenu(tr(Msg::ToolbarColor), colors);
            c.setEnabled(colorful);
            menu.addItem(&c);
        }
        // Drop a trailing separator (options for a tool without any).
        let count = menu.numberOfItems();
        if count > 0 && menu.itemAtIndex(count - 1).is_some_and(|i| i.isSeparatorItem()) {
            menu.removeItemAtIndex(count - 1);
        }
        *self.menu_targets.borrow_mut() = targets;
        let b = self.more.bounds();
        let at = if self.more.isFlipped() {
            objc2_core_foundation::CGPoint::new(0.0, b.size.height + 4.0)
        } else {
            objc2_core_foundation::CGPoint::new(0.0, -4.0)
        };
        menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(&self.more));
    }

    /// Beautify button: accent glyph while beautify is on, a soft pill while
    /// its panel is open.
    pub fn set_beautify_state(&self, open: bool, enabled: bool) {
        let tint = if enabled { crate::util::ns_color(color::ACCENT) } else { NSColor::labelColor() };
        self.beautify.setContentTintColor(Some(&tint));
        without_actions(|| {
            if let Some(l) = self.beautify.layer() {
                let bg = if open { Some(cg_color(color::TOOL_ACTIVE.with_alpha(0.22))) } else { None };
                l.setBackgroundColor(bg.as_deref());
            }
        });
    }

    /// AI writing button: enabled only when AI writing is on in Settings;
    /// the disabled button's tooltip says where to turn it on.
    pub fn set_ai_enabled(&self, enabled: bool) {
        self.ai.setEnabled(enabled);
        let tip = tr(if enabled { Msg::AiMenuTip } else { Msg::AiMenuDisabledTip });
        self.ai.setToolTip(Some(&NSString::from_str(tip)));
    }

    /// Smart Redact button: disabled (with a "working" tooltip) while a
    /// detection runs.
    pub fn set_smart_redact_busy(&self, busy: bool) {
        self.smart_redact.setEnabled(!busy);
        let tip = tr(if busy { Msg::ToolSmartRedactBusy } else { Msg::ToolSmartRedact });
        self.smart_redact.setToolTip(Some(&NSString::from_str(tip)));
    }

    /// The AI button (the menu pops up under it).
    pub fn ai_button(&self) -> &NSButton {
        &self.ai
    }

    /// Reflect the editor's state. Animates the tool pill when the tool
    /// changed.
    pub fn sync(
        &self,
        tool: Tool,
        style: Style,
        options: ToolOptions,
        can_undo: bool,
        can_redo: bool,
        has_selection: bool,
    ) {
        self.last.set(Some((tool, style, options)));
        self.last_has_selection.set(has_selection);
        if self.active.replace(Some(tool)) != Some(tool) {
            if let Some((_, b)) = self.tools.iter().find(|(t, _)| *t == tool) {
                let f = b.frame();
                let to = f.origin.x + f.size.width / 2.0;
                let from = self.pill_x.replace(to);
                spring_scalar(&self.pill, "position.x", from, to, motion::TOOL);
            }
            for (t, b) in &self.tools {
                let tint = if *t == tool { NSColor::whiteColor() } else { NSColor::labelColor() };
                b.setContentTintColor(Some(&tint));
            }
        }
        // Palette selection ring.
        without_actions(|| {
            for (c, b) in &self.swatches {
                if let Some(l) = b.layer() {
                    let selected = *c == style.color;
                    l.setBorderWidth(if selected { 2.5 } else { 0.5 });
                    let ring = if selected { color::ACCENT } else { color::HAIRLINE.with_alpha(0.35) };
                    l.setBorderColor(Some(&cg_color(ring)));
                }
            }
        });
        let custom_selected = !PALETTE.contains(&style.color);
        let tint = if custom_selected { crate::util::ns_rgb8(style.color) } else { NSColor::labelColor() };
        self.custom.setContentTintColor(Some(&tint));
        self.widths.setSelectedSegment(style.width.index() as isize);
        self.opacity.setDoubleValue(f64::from(style.opacity));
        self.opacity.setToolTip(Some(&NSString::from_str(&opacity_tip(style.opacity))));

        let colorful = tool.uses_color() || has_selection;
        let crop = tool == Tool::Crop;
        self.sync_overflowed();
        for (_, b) in &self.swatches {
            b.setEnabled(colorful && !crop);
        }
        self.custom.setEnabled(colorful && !crop);
        self.widths.setEnabled(!crop && tool != Tool::Redact && tool != Tool::Spotlight);
        self.opacity.setEnabled(colorful && !crop);
        self.undo.setEnabled(can_undo);
        self.redo.setEnabled(can_redo);
    }
}

/// Opacity steps offered when the slider is in the "…" menu.
const OPACITY_PRESETS: [u8; 4] = [100, 75, 50, 25];

/// "Opacity: 60%".
fn opacity_tip(percent: u8) -> String {
    lumora_core::i18n::trf(Msg::OpacityTip, &[&percent])
}

/// The contextual checkbox for `tool`: its title and state.
fn toggle_option(tool: Tool, options: ToolOptions) -> Option<(Msg, bool)> {
    match tool {
        Tool::Arrow => Some((Msg::OptionCurved, options.curved_arrow)),
        Tool::Rect => Some((Msg::OptionRounded, options.rounded_rect)),
        Tool::Text => Some((Msg::OptionBackground, options.text_background)),
        _ => None,
    }
}

impl Toolbar {
    /// Show the active tool's options (unless they are in the "…" menu).
    fn sync_options(&self, tool: Tool, options: ToolOptions) {
        let toggle = toggle_option(tool, options);
        if let Some((m, on)) = toggle {
            self.toggle.setTitle(&NSString::from_str(tr(m)));
            self.toggle.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        }
        self.redact.setSelectedSegment(isize::from(options.redact == RedactMode::Blur));
        let shown = self.slot_visible(SlotKind::Options);
        let crop = tool == Tool::Crop;
        fade(&self.toggle, shown && toggle.is_some());
        fade(&self.redact, shown && tool == Tool::Redact);
        fade(&self.apply_crop, shown && crop);
        fade(&self.reset_crop, shown && crop);
    }
}

/// Fade a contextual control in, or hide it at once (hidden controls must
/// not take clicks while fading).
fn fade(view: &NSView, visible: bool) {
    if !visible {
        view.setHidden(true);
        view.setAlphaValue(0.0);
        return;
    }
    if !view.isHidden() && view.alphaValue() >= 1.0 {
        return;
    }
    view.setAlphaValue(0.0);
    view.setHidden(false);
    NSAnimationContext::beginGrouping();
    NSAnimationContext::currentContext().setDuration(motion::HOVER_FADE);
    view.animator().setAlphaValue(1.0);
    NSAnimationContext::endGrouping();
}
