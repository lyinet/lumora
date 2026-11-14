//! Pin to screen: a capture floating above everything, like a sticky note.
//!
//! Drag to move · scroll to zoom (around the pointer) · ⌥-scroll for opacity
//! · double-click or Esc to close.

use std::cell::Cell;

use lumora_core::geometry::{Rect, Size};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSFloatingWindowLevel, NSPanel, NSResponder, NSView};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::NSObject;
use objc2_quartz_core::{kCAGravityResize, CAAutoresizingMask, CALayer};

use crate::events::{EventSink, UiEvent};
use crate::tokens::{motion, radius};
use crate::util::{cf_as_object, cg_rect, make_panel, rect, spring_scalar, PanelSpec, KEY_ESCAPE};

const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 8.0;
const MIN_ALPHA: f64 = 0.15;

pub struct PinViewIvars {
    id: u64,
    sink: EventSink,
    base: Size,
    zoom: Cell<f64>,
    closed: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraPinView"]
    #[ivars = PinViewIvars]
    pub struct PinView;

    unsafe impl NSObjectProtocol for PinView {}

    impl PinView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if event.clickCount() >= 2 {
                self.close();
            } else if let Some(w) = self.window() {
                w.makeKeyWindow();
                w.performWindowDragWithEvent(event);
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if event.keyCode() == KEY_ESCAPE {
                self.close();
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let Some(window) = self.window() else { return };
            // Precise (trackpad) deltas are in points; wheel deltas in lines.
            let dy = if event.hasPreciseScrollingDeltas() { event.scrollingDeltaY() } else { event.scrollingDeltaY() * 8.0 };
            if event.modifierFlags().contains(NSEventModifierFlags::Option) {
                let a = (window.alphaValue() + dy * 0.005).clamp(MIN_ALPHA, 1.0);
                window.setAlphaValue(a);
                return;
            }
            let ivars = self.ivars();
            let zoom = (ivars.zoom.get() * (1.0 + dy * 0.01)).clamp(MIN_ZOOM, MAX_ZOOM);
            ivars.zoom.set(zoom);
            // Keep the point under the cursor fixed while resizing.
            let f = rect(window.frame());
            let mouse = NSEvent::mouseLocation();
            let fx = ((mouse.x - f.min_x()) / f.width()).clamp(0.0, 1.0);
            let fy = ((mouse.y - f.min_y()) / f.height()).clamp(0.0, 1.0);
            let (w, h) = (ivars.base.width * zoom, ivars.base.height * zoom);
            let frame = CGRect::new(CGPoint::new(mouse.x - fx * w, mouse.y - fy * h), CGSize::new(w, h));
            window.setFrame_display(frame, true);
        }
    }
);

impl PinView {
    fn close(&self) {
        if self.ivars().closed.replace(true) {
            return;
        }
        if let Some(w) = self.window() {
            w.orderOut(None);
        }
        (self.ivars().sink)(UiEvent::PinClosed(self.ivars().id));
    }
}

/// A pinned capture. Dropping it closes the window.
pub struct PinWindow {
    panel: Retained<NSPanel>,
    _view: Retained<PinView>,
}

impl std::fmt::Debug for PinWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PinWindow")
    }
}

impl PinWindow {
    /// Pin `image` (which covers `frame`, AppKit global points) in place.
    pub fn show(mtm: MainThreadMarker, id: u64, image: CFRetained<CGImage>, frame: Rect, sink: EventSink) -> Self {
        let panel = make_panel(
            mtm,
            cg_rect(frame),
            PanelSpec { level: NSFloatingWindowLevel, keyable: true, shadow: true, ignores_mouse: false },
        );
        let local = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(frame.width(), frame.height()));
        let this = PinView::alloc(mtm).set_ivars(PinViewIvars {
            id,
            sink,
            base: frame.size,
            zoom: Cell::new(1.0),
            closed: Cell::new(false),
        });
        let view: Retained<PinView> = unsafe { msg_send![super(this), initWithFrame: local] };
        let root = CALayer::new();
        let img = CALayer::new();
        img.setFrame(local);
        img.setAutoresizingMask(CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable);
        img.setContentsGravity(unsafe { kCAGravityResize });
        img.setCornerRadius(radius::PIN);
        img.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        img.setMasksToBounds(true);
        // SAFETY: CGImage is a valid layer `contents`.
        unsafe { img.setContents(Some(cf_as_object(&*image))) };
        root.addSublayer(&img);
        view.setLayer(Some(&root));
        view.setWantsLayer(true);
        panel.setContentView(Some(&view));
        panel.orderFrontRegardless();
        panel.makeKeyWindow();
        panel.makeFirstResponder(Some(&view));
        panel.invalidateShadow();
        // A gentle "drop" onto the screen.
        spring_scalar(&img, "transform.scale", 1.04, 1.0, motion::POP);
        PinWindow { panel, _view: view }
    }
}

impl Drop for PinWindow {
    fn drop(&mut self) {
        self.panel.orderOut(None);
    }
}
