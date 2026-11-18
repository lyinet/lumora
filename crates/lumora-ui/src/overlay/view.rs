//! The per-screen overlay view: translates `NSEvent`s into
//! `lumora_core::selection::Input`s and renders [`Frame`]s into a layer tree.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Weak;

use lumora_core::geometry::{Handle, Point, Rect};
use lumora_core::selection::{CursorHint, Direction, Input, Key, Mods};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSCursor, NSEvent, NSEventModifierFlags, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
use objc2_core_graphics::CGImage;
use objc2_quartz_core::{kCAFillRuleEvenOdd, kCAFilterNearest, kCAGravityResize, CALayer, CAShapeLayer, CATextLayer};

use super::Inner;
use crate::events::OverlayMode;
use crate::tokens::{color, radius, size, typography};
use crate::util::{
    cf_as_object, cg_color, cg_rect, cutout_path, line_path, mono_digits, rect_path, round_rect_path, set_text,
    text_layer, weight, without_actions,
};

/// Everything a view needs to draw one frame, in **global** Quartz points.
pub(crate) struct Frame {
    pub selection: Option<Rect>,
    pub highlight: Option<Rect>,
    pub show_handles: bool,
    pub pop_handles: bool,
    pub mouse: Point,
    pub crosshair: bool,
    pub pill: Option<(String, Rect)>,
    pub loupe: Option<LoupeFrame>,
    pub hint: bool,
}

pub(crate) struct LoupeFrame {
    /// Container frame (global).
    pub frame: Rect,
    pub image: CFRetained<CGImage>,
    pub label: String,
}

/// The layer tree of one overlay view.
struct Layers {
    dim: Retained<CAShapeLayer>,
    highlight: Retained<CAShapeLayer>,
    selection: Retained<CAShapeLayer>,
    handles: Vec<Retained<CAShapeLayer>>,
    cross_h: Retained<CAShapeLayer>,
    cross_v: Retained<CAShapeLayer>,
    pill: Retained<CALayer>,
    pill_text: Retained<CATextLayer>,
    loupe: Retained<CALayer>,
    loupe_image: Retained<CALayer>,
    loupe_label: Retained<CATextLayer>,
    hint: Retained<CALayer>,
}

pub struct OverlayViewIvars {
    controller: RefCell<Weak<Inner>>,
    layers: OnceCell<Layers>,
    /// This view's screen in global Quartz points.
    screen: Cell<Rect>,
    tracking: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraOverlayView"]
    #[ivars = OverlayViewIvars]
    pub struct OverlayView;

    unsafe impl NSObjectProtocol for OverlayView {}

    impl OverlayView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(old) = self.ivars().tracking.borrow_mut().take() {
                self.removeTrackingArea(&old);
            }
            let opts = NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect
                | NSTrackingAreaOptions::MouseEnteredAndExited;
            // SAFETY: owner is self, which outlives the tracking area.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    opts,
                    Some(self as &AnyObject),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().tracking.borrow_mut() = Some(area);
            // SAFETY: calling super's implementation.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.send(Input::MouseMoved(self.global_point(event)));
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            // Take key focus when the pointer moves onto this screen so keys
            // keep working across displays.
            if let Some(w) = self.window() {
                w.makeKeyWindow();
                w.makeFirstResponder(Some(self));
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let clicks = event.clickCount().max(1) as u32;
            self.send(Input::MouseDown { at: self.global_point(event), clicks });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.send(Input::MouseDragged(self.global_point(event)));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.send(Input::MouseUp(self.global_point(event)));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            self.send(Input::KeyDown(Key::Escape));
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if let Some(k) = map_key(event.keyCode()) {
                self.send(Input::KeyDown(k));
            }
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            if let Some(k) = map_key(event.keyCode()) {
                self.send(Input::KeyUp(k));
            }
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            let f = event.modifierFlags();
            self.send(Input::ModifiersChanged(Mods {
                shift: f.contains(NSEventModifierFlags::Shift),
                option: f.contains(NSEventModifierFlags::Option),
            }));
        }
    }
);

/// Carbon virtual key codes → overlay keys.
fn map_key(code: u16) -> Option<Key> {
    Some(match code {
        53 => Key::Escape,
        36 | 76 => Key::Enter,
        49 => Key::Space,
        123 => Key::Arrow(Direction::Left),
        124 => Key::Arrow(Direction::Right),
        125 => Key::Arrow(Direction::Down),
        126 => Key::Arrow(Direction::Up),
        8 => Key::CopyColor,
        _ => return None,
    })
}

fn shape(fill: Option<crate::tokens::Rgba>, stroke: Option<crate::tokens::Rgba>, line: f64) -> Retained<CAShapeLayer> {
    let l = CAShapeLayer::new();
    l.setFillColor(fill.map(cg_color).as_deref());
    l.setStrokeColor(stroke.map(cg_color).as_deref());
    l.setLineWidth(line);
    l
}

impl OverlayView {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        mtm: MainThreadMarker,
        frame: CGRect,
        screen: Rect,
        scale: f64,
        backdrop: Option<&CGImage>,
        mode: OverlayMode,
        hint: &str,
        controller: Weak<Inner>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(OverlayViewIvars {
            controller: RefCell::new(controller),
            layers: OnceCell::new(),
            screen: Cell::new(screen),
            tracking: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let root = CALayer::new();
        root.setGeometryFlipped(true);
        this.setLayer(Some(&root));
        this.setWantsLayer(true);
        let _ = this.ivars().layers.set(build_layers(&root, screen, scale, backdrop, mode, hint));
        this
    }

    fn global_point(&self, event: &NSEvent) -> Point {
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let s = self.ivars().screen.get();
        Point::new(local.x + s.min_x(), local.y + s.min_y())
    }

    fn send(&self, input: Input) {
        if let Some(inner) = self.ivars().controller.borrow().upgrade() {
            Inner::handle_input(&inner, input);
        }
    }

    /// Fade the dim layer in.
    pub(crate) fn appear(&self) {
        if let Some(l) = self.ivars().layers.get() {
            crate::util::ease_scalar(&l.dim, "opacity", 0.0, 1.0, crate::tokens::motion::DIM_FADE);
        }
    }

    /// Fade everything out.
    pub(crate) fn disappear(&self) {
        if let Some(root) = self.layer() {
            crate::util::ease_scalar(&root, "opacity", 1.0, 0.0, crate::tokens::motion::OVERLAY_OUT);
        }
    }

    // The directional resize cursors are deprecated in macOS 15 in favour of
    // `frameResizeCursorFromPosition:inDirections:`, which doesn't exist on
    // our macOS 14 floor.
    #[allow(deprecated)]
    pub(crate) fn set_cursor(hint: CursorHint) {
        let c = match hint {
            CursorHint::Crosshair => NSCursor::crosshairCursor(),
            CursorHint::Move => NSCursor::openHandCursor(),
            CursorHint::Resize(Handle::Left | Handle::Right) => NSCursor::resizeLeftRightCursor(),
            CursorHint::Resize(Handle::Top | Handle::Bottom) => NSCursor::resizeUpDownCursor(),
            CursorHint::Resize(_) => NSCursor::crosshairCursor(),
        };
        c.set();
    }

    /// Render one frame. All layer updates happen without implicit actions;
    /// motion comes from the controller's springs, not CA defaults.
    pub(crate) fn render(&self, f: &Frame) {
        let Some(l) = self.ivars().layers.get() else { return };
        let screen = self.ivars().screen.get();
        let bounds = Rect::new(0.0, 0.0, screen.width(), screen.height());
        let local = |r: Rect| r.relative_to(&screen);
        let lp = Point::new(f.mouse.x - screen.min_x(), f.mouse.y - screen.min_y());
        let on_this_screen = screen.contains(f.mouse);

        without_actions(|| {
            // Dim with a cutout for the selection (or the hovered window).
            let hole = f.selection.or(f.highlight).map(local).and_then(|r| r.intersection(&bounds));
            l.dim.setPath(Some(&cutout_path(bounds, hole)));

            // Window highlight (only without a selection).
            match f.highlight.filter(|_| f.selection.is_none()).map(local) {
                Some(r) => {
                    l.highlight.setHidden(false);
                    l.highlight.setPath(Some(&round_rect_path(r.inset(0.5, 0.5), 10.0)));
                }
                None => l.highlight.setHidden(true),
            }

            // Selection outline and handles.
            match f.selection.map(local) {
                Some(r) => {
                    l.selection.setHidden(false);
                    l.selection.setPath(Some(&rect_path(r.inset(-0.5, -0.5))));
                    for (layer, h) in l.handles.iter().zip(Handle::ALL) {
                        let p = r.handle_point(h);
                        layer.setPosition(CGPoint::new(p.x, p.y));
                        layer.setHidden(!f.show_handles);
                    }
                }
                None => {
                    l.selection.setHidden(true);
                    l.handles.iter().for_each(|h| h.setHidden(true));
                }
            }

            // Crosshair guides through the pointer.
            let show_cross = f.crosshair && on_this_screen;
            l.cross_h.setHidden(!show_cross);
            l.cross_v.setHidden(!show_cross);
            if show_cross {
                let x = lp.x.floor() + 0.5;
                let y = lp.y.floor() + 0.5;
                l.cross_h.setPath(Some(&line_path((0.0, y), (bounds.width(), y))));
                l.cross_v.setPath(Some(&line_path((x, 0.0), (x, bounds.height()))));
            }

            // Dimension / coordinate pill.
            match &f.pill {
                Some((text, frame)) if screen.intersects(frame) => {
                    l.pill.setHidden(false);
                    l.pill.setFrame(cg_rect(local(*frame)));
                    set_text(&l.pill_text, text);
                    let fr = local(*frame);
                    let text_h = typography::PILL + 4.0;
                    l.pill_text.setFrame(cg_rect(Rect::new(0.0, (fr.height() - text_h) / 2.0, fr.width(), text_h)));
                }
                _ => l.pill.setHidden(true),
            }

            // Magnifier loupe.
            match &f.loupe {
                Some(loupe) if on_this_screen => {
                    l.loupe.setHidden(false);
                    l.loupe.setFrame(cg_rect(local(loupe.frame)));
                    // SAFETY: a CGImage is a valid `contents` value.
                    unsafe { l.loupe_image.setContents(Some(cf_as_object(&*loupe.image))) };
                    set_text(&l.loupe_label, &loupe.label);
                }
                _ => l.loupe.setHidden(true),
            }

            l.hint.setHidden(!f.hint);
        });

        if f.pop_handles && f.selection.is_some() {
            for h in &l.handles {
                crate::util::spring_scalar(h, "transform.scale", 0.3, 1.0, crate::tokens::motion::POP);
            }
        }
    }
}

fn build_layers(
    root: &CALayer,
    screen: Rect,
    scale: f64,
    backdrop: Option<&CGImage>,
    mode: OverlayMode,
    hint: &str,
) -> Layers {
    let bounds = Rect::new(0.0, 0.0, screen.width(), screen.height());
    let accent = match mode {
        OverlayMode::Capture | OverlayMode::Text | OverlayMode::Scroll => color::ACCENT,
        OverlayMode::Record => color::RECORD,
    };

    // Frozen frame (capture) or nothing (record: live desktop shows through).
    let backdrop_layer = CALayer::new();
    backdrop_layer.setFrame(cg_rect(bounds));
    backdrop_layer.setContentsGravity(unsafe { kCAGravityResize });
    if let Some(img) = backdrop {
        // SAFETY: CGImage is a valid `contents` value.
        unsafe { backdrop_layer.setContents(Some(cf_as_object(img))) };
    }
    root.addSublayer(&backdrop_layer);

    let dim = shape(Some(color::DIM), None, 0.0);
    dim.setFillRule(unsafe { kCAFillRuleEvenOdd });
    dim.setFrame(cg_rect(bounds));
    dim.setOpacity(0.0);
    root.addSublayer(&dim);

    let highlight = shape(Some(accent.with_alpha(color::HIGHLIGHT_FILL.a)), Some(accent.with_alpha(0.9)), 2.0);
    highlight.setHidden(true);
    root.addSublayer(&highlight);

    let selection = shape(None, Some(color::SELECTION_STROKE), 1.0);
    selection.setShadowColor(Some(&cg_color(color::SHADOW)));
    selection.setShadowOpacity(0.35);
    selection.setShadowRadius(2.0);
    selection.setShadowOffset(objc2_core_foundation::CGSize::new(0.0, 0.0));
    selection.setHidden(true);
    root.addSublayer(&selection);

    let handles = (0..8)
        .map(|_| {
            let h = shape(Some(color::HANDLE_FILL), Some(accent), 1.5);
            let d = size::HANDLE;
            h.setBounds(cg_rect(Rect::new(0.0, 0.0, d, d)));
            h.setPath(Some(&round_rect_path(Rect::new(0.0, 0.0, d, d), d / 2.0)));
            h.setShadowColor(Some(&cg_color(color::SHADOW)));
            h.setShadowOpacity(0.3);
            h.setShadowRadius(1.5);
            h.setHidden(true);
            root.addSublayer(&h);
            h
        })
        .collect();

    let cross_h = shape(None, Some(color::CROSSHAIR), 1.0);
    let cross_v = shape(None, Some(color::CROSSHAIR), 1.0);
    for c in [&cross_h, &cross_v] {
        c.setLineDashPattern(Some(&objc2_foundation::NSArray::from_retained_slice(&[
            objc2_foundation::NSNumber::numberWithDouble(4.0),
            objc2_foundation::NSNumber::numberWithDouble(3.0),
        ])));
        c.setHidden(true);
        root.addSublayer(c);
    }

    // Pill: dark glass with monospaced digits.
    let pill = CALayer::new();
    pill.setBackgroundColor(Some(&cg_color(color::PILL_BG)));
    pill.setCornerRadius(radius::PILL);
    pill.setBorderWidth(0.5);
    pill.setBorderColor(Some(&cg_color(color::HAIRLINE)));
    pill.setHidden(true);
    let pill_font = mono_digits(typography::PILL, weight::MEDIUM);
    let pill_text = text_layer(&pill_font, typography::PILL, color::PILL_TEXT, scale);
    pill.addSublayer(&pill_text);
    root.addSublayer(&pill);

    // Loupe: nearest-neighbour magnified pixels, grid, centre marker, label.
    let grid = size::LOUPE_GRID as f64;
    let side = grid * size::LOUPE_CELL;
    let loupe = CALayer::new();
    loupe.setHidden(true);
    loupe.setShadowColor(Some(&cg_color(color::SHADOW)));
    loupe.setShadowOpacity(0.35);
    loupe.setShadowRadius(10.0);
    loupe.setShadowOffset(objc2_core_foundation::CGSize::new(0.0, 4.0));
    let clip = CALayer::new();
    clip.setFrame(cg_rect(Rect::new(0.0, 0.0, side, side)));
    clip.setCornerRadius(radius::LOUPE);
    clip.setMasksToBounds(true);
    clip.setBorderWidth(1.0);
    clip.setBorderColor(Some(&cg_color(color::SELECTION_STROKE)));
    let loupe_image = CALayer::new();
    loupe_image.setFrame(cg_rect(Rect::new(0.0, 0.0, side, side)));
    loupe_image.setMagnificationFilter(unsafe { kCAFilterNearest });
    loupe_image.setContentsGravity(unsafe { kCAGravityResize });
    clip.addSublayer(&loupe_image);
    let loupe_grid = shape(None, Some(color::LOUPE_GRID), 0.5);
    {
        let path = objc2_core_graphics::CGMutablePath::new();
        for i in 1..size::LOUPE_GRID {
            let o = i as f64 * size::LOUPE_CELL;
            // SAFETY: valid mutable path, identity transform.
            unsafe {
                objc2_core_graphics::CGMutablePath::move_to_point(Some(&path), std::ptr::null(), o, 0.0);
                objc2_core_graphics::CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), o, side);
                objc2_core_graphics::CGMutablePath::move_to_point(Some(&path), std::ptr::null(), 0.0, o);
                objc2_core_graphics::CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), side, o);
            }
        }
        loupe_grid.setPath(Some(&path));
    }
    clip.addSublayer(&loupe_grid);
    let c0 = (grid / 2.0).floor() * size::LOUPE_CELL;
    let loupe_center = shape(None, Some(color::LOUPE_CENTER), 1.5);
    loupe_center.setPath(Some(&rect_path(Rect::new(c0, c0, size::LOUPE_CELL, size::LOUPE_CELL))));
    loupe_center.setShadowColor(Some(&cg_color(color::SHADOW)));
    loupe_center.setShadowOpacity(0.6);
    loupe_center.setShadowRadius(1.0);
    clip.addSublayer(&loupe_center);
    loupe.addSublayer(&clip);
    let label_bg = CALayer::new();
    label_bg.setFrame(cg_rect(Rect::new(0.0, side + 6.0, side, size::LOUPE_LABEL_HEIGHT)));
    label_bg.setBackgroundColor(Some(&cg_color(color::PILL_BG)));
    label_bg.setCornerRadius(radius::PILL);
    let hex_font = mono_digits(typography::LOUPE_HEX, weight::MEDIUM);
    let loupe_label = text_layer(&hex_font, typography::LOUPE_HEX, color::PILL_TEXT, scale);
    loupe_label.setFrame(cg_rect(Rect::new(0.0, (size::LOUPE_LABEL_HEIGHT - 15.0) / 2.0, side, 15.0)));
    label_bg.addSublayer(&loupe_label);
    loupe.addSublayer(&label_bg);
    root.addSublayer(&loupe);

    // Hint pill, top centre.
    let hint_text = hint;
    let hint_font = objc2_app_kit::NSFont::systemFontOfSize_weight(typography::CAPTION, weight::MEDIUM);
    let w = crate::util::text_width(hint_text, &hint_font) + 2.0 * size::PILL_PADDING + 8.0;
    let hint = CALayer::new();
    hint.setFrame(cg_rect(Rect::new((bounds.width() - w) / 2.0, 46.0, w, size::PILL_HEIGHT + 2.0)));
    hint.setBackgroundColor(Some(&cg_color(color::PILL_BG)));
    hint.setCornerRadius((size::PILL_HEIGHT + 2.0) / 2.0);
    hint.setBorderWidth(0.5);
    hint.setBorderColor(Some(&cg_color(color::HAIRLINE)));
    let hint_label = text_layer(&hint_font, typography::CAPTION, color::PILL_TEXT_SECONDARY, scale);
    set_text(&hint_label, hint_text);
    hint_label.setFrame(cg_rect(Rect::new(0.0, (size::PILL_HEIGHT + 2.0 - 15.0) / 2.0, w, 15.0)));
    hint.addSublayer(&hint_label);
    root.addSublayer(&hint);

    Layers {
        dim,
        highlight,
        selection,
        handles,
        cross_h,
        cross_v,
        pill,
        pill_text,
        loupe,
        loupe_image,
        loupe_label,
        hint,
    }
}
