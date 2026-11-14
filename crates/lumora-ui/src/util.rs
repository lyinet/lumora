//! Shared AppKit / Core Animation helpers.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use lumora_core::geometry::{flip_y, Rect};
use lumora_core::spring::Spring;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSStringDrawing;
use objc2_app_kit::{NSColor, NSFont, NSFontWeight, NSPanel, NSResponder, NSScreen, NSWindow};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath, CGPath};
use objc2_foundation::{NSNumber, NSObjectNSKeyValueCoding, NSString, NSTimer};
use objc2_quartz_core::{
    kCAFillModeForwards, CABasicAnimation, CALayer, CAMediaTiming, CASpringAnimation, CATextLayer, CATransaction,
};

use crate::tokens::{motion, Rgba};

// ---------------------------------------------------------------------------
// Colours, fonts, geometry
// ---------------------------------------------------------------------------

pub fn cg_color(c: Rgba) -> CFRetained<CGColor> {
    CGColor::new_srgb(c.r, c.g, c.b, c.a)
}

pub fn ns_color(c: Rgba) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c.r, c.g, c.b, c.a)
}

pub fn cg_rect(r: Rect) -> CGRect {
    CGRect::new(CGPoint::new(r.min_x(), r.min_y()), CGSize::new(r.width(), r.height()))
}

pub fn rect(r: CGRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// System font with monospaced digits (for live numbers).
pub fn mono_digits(size: CGFloat, weight: NSFontWeight) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
}

/// Font weights from `NSFontWeight*` (values are stable across releases).
pub mod weight {
    pub const REGULAR: f64 = 0.0;
    pub const MEDIUM: f64 = 0.23;
    pub const SEMIBOLD: f64 = 0.3;
    pub const BOLD: f64 = 0.4;
}

// ---------------------------------------------------------------------------
// Screens and coordinate spaces
// ---------------------------------------------------------------------------

/// Height of the primary display (the screen at AppKit origin), used to flip
/// between AppKit (bottom-left) and Quartz (top-left) global coordinates.
pub fn primary_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm).firstObject().map(|s| s.frame().size.height).unwrap_or(0.0)
}

/// A screen's frame in global Quartz (top-left) points.
pub fn quartz_frame(screen: &NSScreen, primary_height: f64) -> Rect {
    flip_y(rect(screen.frame()), primary_height)
}

/// AppKit global rect → Quartz global rect (and back; it's an involution).
pub fn flip(r: Rect, mtm: MainThreadMarker) -> Rect {
    flip_y(r, primary_height(mtm))
}

/// `CGDirectDisplayID` of an `NSScreen`.
pub fn screen_id(screen: &NSScreen) -> u32 {
    let desc = screen.deviceDescription();
    let key = NSString::from_str("NSScreenNumber");
    desc.objectForKey(&key).and_then(|v| v.downcast::<NSNumber>().ok()).map(|n| n.unsignedIntValue()).unwrap_or(0)
}

/// The screen containing the mouse pointer, else the main screen.
pub fn screen_under_mouse(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    let p = objc2_app_kit::NSEvent::mouseLocation();
    NSScreen::screens(mtm)
        .iter()
        .find(|s| rect(s.frame()).contains(lumora_core::Point::new(p.x, p.y)))
        .or_else(|| NSScreen::mainScreen(mtm))
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

pub fn rect_path(r: Rect) -> CFRetained<CGPath> {
    // SAFETY: a null transform means identity.
    unsafe { CGPath::with_rect(cg_rect(r), std::ptr::null()) }
}

pub fn round_rect_path(r: Rect, radius: f64) -> CFRetained<CGPath> {
    let radius = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    // SAFETY: radius clamped to the rect; null transform is identity.
    unsafe { CGPath::with_rounded_rect(cg_rect(r), radius, radius, std::ptr::null()) }
}

/// `outer` with `hole` punched out (draw with even-odd fill).
pub fn cutout_path(outer: Rect, hole: Option<Rect>) -> CFRetained<CGPath> {
    let path = CGMutablePath::new();
    // SAFETY: valid mutable path, identity transform.
    unsafe {
        CGMutablePath::add_rect(Some(&path), std::ptr::null(), cg_rect(outer));
        if let Some(h) = hole {
            CGMutablePath::add_rect(Some(&path), std::ptr::null(), cg_rect(h));
        }
    }
    // SAFETY: CGMutablePath is a subtype of CGPath.
    unsafe { CFRetained::cast_unchecked(path) }
}

/// A straight line segment path.
pub fn line_path(a: (f64, f64), b: (f64, f64)) -> CFRetained<CGPath> {
    let path = CGMutablePath::new();
    // SAFETY: valid mutable path, identity transform.
    unsafe {
        CGMutablePath::move_to_point(Some(&path), std::ptr::null(), a.0, a.1);
        CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), b.0, b.1);
    }
    // SAFETY: CGMutablePath is a subtype of CGPath.
    unsafe { CFRetained::cast_unchecked(path) }
}

/// View a CF object as an `AnyObject` (all CF types are valid ObjC objects).
pub fn cf_as_object<T: ?Sized>(cf: &T) -> &AnyObject {
    // SAFETY: CoreFoundation objects are toll-free usable as `id`.
    unsafe { &*(cf as *const T as *const AnyObject) }
}

// ---------------------------------------------------------------------------
// Core Animation
// ---------------------------------------------------------------------------

/// Run `f` with implicit layer animations disabled (for live tracking).
pub fn without_actions<R>(f: impl FnOnce() -> R) -> R {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let r = f();
    CATransaction::commit();
    r
}

/// Build a `CASpringAnimation` from a [`Spring`], with its duration set to
/// the spring's own settling time (so it never cuts off early).
pub fn spring_animation(
    key_path: &str,
    from: Option<&AnyObject>,
    to: &AnyObject,
    spring: Spring,
) -> Retained<CASpringAnimation> {
    let anim = CASpringAnimation::animationWithKeyPath(Some(&NSString::from_str(key_path)));
    anim.setMass(spring.mass);
    anim.setStiffness(spring.stiffness);
    anim.setDamping(spring.damping);
    anim.setInitialVelocity(spring.initial_velocity);
    anim.setDuration(spring.settling_time(motion::SETTLE_EPSILON));
    // SAFETY: from/to are valid objects of the type the key path expects
    // (NSNumber for scalars, CGPath for paths) — guaranteed by callers.
    unsafe {
        anim.setFromValue(from);
        anim.setToValue(Some(to));
    }
    anim
}

/// Spring a scalar layer property (`opacity`, `transform.scale`,
/// `position.x`…) to `to`, updating the model value so it sticks.
pub fn spring_scalar(layer: &CALayer, key_path: &str, from: f64, to: f64, spring: Spring) {
    let from_n = NSNumber::numberWithDouble(from);
    let to_n = NSNumber::numberWithDouble(to);
    let anim = spring_animation(key_path, Some(&from_n), &to_n, spring);
    without_actions(|| {
        // SAFETY: setValue:forKeyPath: with an NSNumber on an animatable key.
        unsafe { layer.setValue_forKeyPath(Some(&to_n), &NSString::from_str(key_path)) };
    });
    layer.addAnimation_forKey(&anim, Some(&NSString::from_str(key_path)));
}

/// Ease-out basic animation of a scalar property, updating the model value.
pub fn ease_scalar(layer: &CALayer, key_path: &str, from: f64, to: f64, duration: f64) {
    let anim = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str(key_path)));
    let from_n = NSNumber::numberWithDouble(from);
    let to_n = NSNumber::numberWithDouble(to);
    // SAFETY: NSNumber values for a scalar key path.
    unsafe {
        anim.setFromValue(Some(&from_n));
        anim.setToValue(Some(&to_n));
        anim.setFillMode(kCAFillModeForwards);
    }
    anim.setDuration(duration);
    anim.setTimingFunction(Some(&objc2_quartz_core::CAMediaTimingFunction::functionWithName(unsafe {
        objc2_quartz_core::kCAMediaTimingFunctionEaseOut
    })));
    without_actions(|| unsafe { layer.setValue_forKeyPath(Some(&to_n), &NSString::from_str(key_path)) });
    layer.addAnimation_forKey(&anim, Some(&NSString::from_str(key_path)));
}

/// Run `f` and call `done` when all animations added inside complete.
pub fn with_completion(f: impl FnOnce(), done: impl Fn() + 'static) {
    CATransaction::begin();
    let block = RcBlock::new(done);
    // SAFETY: block is a valid `void (^)(void)`.
    unsafe { CATransaction::setCompletionBlock(Some(&block)) };
    f();
    CATransaction::commit();
}

/// A text layer preconfigured for crisp HUD text.
pub fn text_layer(font: &NSFont, size: f64, color: Rgba, scale: f64) -> Retained<CATextLayer> {
    let t = CATextLayer::new();
    // SAFETY: NSFont is toll-free bridged to CTFont, which CATextLayer takes.
    unsafe { t.setFont(Some(&*(font as *const NSFont as *const objc2_core_foundation::CFType))) };
    t.setFontSize(size);
    t.setForegroundColor(Some(&cg_color(color)));
    t.setAlignmentMode(unsafe { objc2_quartz_core::kCAAlignmentCenter });
    t.setContentsScale(scale);
    t
}

pub fn set_text(layer: &CATextLayer, s: &str) {
    // SAFETY: NSString is a valid `string` value.
    unsafe { layer.setString(Some(&NSString::from_str(s))) };
}

/// Width of `s` in `font`, for sizing pills around text.
pub fn text_width(s: &str, font: &NSFont) -> f64 {
    use objc2_foundation::NSDictionary;
    let key = unsafe { objc2_app_kit::NSFontAttributeName };
    let attrs = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[&**font]);
    // SAFETY: attribute dictionary with an NSFont value.
    unsafe { NSString::from_str(s).sizeWithAttributes(Some(&attrs)) }.width.ceil()
}

// ---------------------------------------------------------------------------
// Timers
// ---------------------------------------------------------------------------

/// Schedule a main-run-loop timer. The closure returns `false` to stop a
/// repeating timer. Returns the timer so callers can `invalidate()` early.
pub fn schedule_timer(interval: f64, repeats: bool, f: impl FnMut() -> bool + 'static) -> Retained<NSTimer> {
    let f = RefCell::new(f);
    let block = RcBlock::new(move |timer: NonNull<NSTimer>| {
        let keep_going = match f.try_borrow_mut() {
            Ok(mut f) => f(),
            Err(_) => true, // re-entrant tick; skip
        };
        if !keep_going {
            // SAFETY: the timer pointer is valid during its own callback.
            unsafe { timer.as_ref() }.invalidate();
        }
    });
    // SAFETY: valid block; scheduled on the current (main) run loop.
    let timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(interval, repeats, &block) };
    // Keep firing while menus are tracking or windows are dragged.
    // SAFETY: NSRunLoopCommonModes is a valid static mode name.
    unsafe {
        objc2_foundation::NSRunLoop::currentRunLoop().addTimer_forMode(&timer, objc2_foundation::NSRunLoopCommonModes)
    };
    timer
}

/// Seconds on a monotonic clock (`CACurrentMediaTime`).
pub fn now() -> f64 {
    objc2_quartz_core::CACurrentMediaTime()
}

// ---------------------------------------------------------------------------
// Target/action → closure bridge
// ---------------------------------------------------------------------------

pub struct ActionIvars {
    handler: Box<dyn Fn(&AnyObject)>,
}

define_class!(
    /// An `NSObject` whose `perform:` selector calls a Rust closure, so
    /// buttons/menu items can use closures as their target/action.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraActionTarget"]
    #[ivars = ActionIvars]
    pub struct ActionTarget;

    unsafe impl NSObjectProtocol for ActionTarget {}

    impl ActionTarget {
        #[unsafe(method(perform:))]
        fn perform(&self, sender: &AnyObject) {
            (self.ivars().handler)(sender);
        }
    }
);

impl ActionTarget {
    pub fn new(mtm: MainThreadMarker, handler: impl Fn(&AnyObject) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ActionIvars { handler: Box::new(handler) });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    /// The selector to pair with this target.
    pub fn action() -> objc2::runtime::Sel {
        sel!(perform:)
    }
}

// ---------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------

define_class!(
    /// Borderless panels return NO from `canBecomeKeyWindow` by default, so
    /// they never receive key events. This subclass opts in (but never
    /// becomes *main*), which is what overlays, pins and the countdown need
    /// to handle Esc/Enter without activating the app.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraKeyablePanel"]
    pub struct KeyablePanel;

    impl KeyablePanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }
);

/// Options for [`make_panel`].
#[derive(Debug, Clone, Copy)]
pub struct PanelSpec {
    pub level: isize,
    pub keyable: bool,
    pub shadow: bool,
    pub ignores_mouse: bool,
}

/// A transparent, borderless, non-activating panel at `frame` (AppKit
/// global points), visible on all Spaces and over full-screen apps.
pub fn make_panel(mtm: MainThreadMarker, frame: CGRect, spec: PanelSpec) -> Retained<NSPanel> {
    use objc2_app_kit::{NSBackingStoreType, NSWindowCollectionBehavior, NSWindowStyleMask};
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel: Retained<NSPanel> = if spec.keyable {
        let p: Retained<KeyablePanel> = unsafe {
            msg_send![KeyablePanel::alloc(mtm), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        Retained::into_super(p)
    } else {
        NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: we own the panel through `Retained`; AppKit must not release
    // it again on close.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setLevel(spec.level);
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(spec.shadow);
    panel.setIgnoresMouseEvents(spec.ignores_mouse);
    panel.setHidesOnDeactivate(false);
    panel.setFloatingPanel(true);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel
}

/// A layer-hosting view of `size` whose root layer uses y-down geometry,
/// matching `lumora_core`'s coordinate convention.
pub fn layer_host_view(mtm: MainThreadMarker, frame: CGRect) -> (Retained<objc2_app_kit::NSView>, Retained<CALayer>) {
    let view = objc2_app_kit::NSView::initWithFrame(objc2_app_kit::NSView::alloc(mtm), frame);
    let root = CALayer::new();
    root.setGeometryFlipped(true);
    view.setLayer(Some(&root));
    view.setWantsLayer(true);
    (view, root)
}

/// A rounded "glass" HUD background (`NSVisualEffectView`, HUD material,
/// always dark) filling `frame`.
pub fn glass_view(mtm: MainThreadMarker, frame: CGRect, radius: f64) -> Retained<objc2_app_kit::NSVisualEffectView> {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    };
    let v = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
    v.setMaterial(NSVisualEffectMaterial::HUDWindow);
    v.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    v.setState(NSVisualEffectState::Active);
    // SAFETY: the appearance name constant is valid.
    if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
        v.setAppearance(Some(&dark));
    }
    v.setWantsLayer(true);
    if let Some(layer) = v.layer() {
        layer.setCornerRadius(radius);
        layer.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        layer.setMasksToBounds(true);
        layer.setBorderWidth(0.5);
        layer.setBorderColor(Some(&cg_color(crate::tokens::color::HAIRLINE)));
    }
    v
}

// ---------------------------------------------------------------------------
// Key catching view
// ---------------------------------------------------------------------------

pub struct KeyCatcherIvars {
    on_key: Box<dyn Fn(u16)>,
}

define_class!(
    /// A layer-hosting view that becomes first responder and forwards key
    /// codes to a closure (Esc on the countdown, etc.).
    #[unsafe(super(objc2_app_kit::NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraKeyCatcherView"]
    #[ivars = KeyCatcherIvars]
    pub struct KeyCatcherView;

    impl KeyCatcherView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &objc2_app_kit::NSEvent) {
            (self.ivars().on_key)(event.keyCode());
        }
    }
);

impl KeyCatcherView {
    /// Create with a y-down root layer (returned alongside).
    pub fn new(
        mtm: MainThreadMarker,
        frame: CGRect,
        on_key: impl Fn(u16) + 'static,
    ) -> (Retained<Self>, Retained<CALayer>) {
        let this = Self::alloc(mtm).set_ivars(KeyCatcherIvars { on_key: Box::new(on_key) });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let root = CALayer::new();
        root.setGeometryFlipped(true);
        this.setLayer(Some(&root));
        this.setWantsLayer(true);
        (this, root)
    }
}

/// Key code for Escape.
pub const KEY_ESCAPE: u16 = 53;
