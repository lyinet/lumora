//! Click visualisation while recording.
//!
//! * [`ClickMonitor`] — global + local mouse-down monitors (`NSEvent`), so
//!   clicks in other apps *and* on Lumora's own windows are seen.
//! * [`ClickOverlay`] — one transparent, click-through, all-Spaces panel per
//!   display that draws a springy, fading ring at each click.
//!
//! Unlike the rest of the recording chrome, the overlay panels are meant to
//! be **in** the video: the record flow passes [`ClickOverlay::window_numbers`]
//! to the recorder, which leaves them out of the `SCContentFilter`'s
//! excluded windows.

use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use lumora_core::click_log::MouseButton;
use lumora_core::geometry::{Point, Rect};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSPanel, NSScreen, NSScreenSaverWindowLevel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGPath;
use objc2_quartz_core::{CALayer, CAShapeLayer};

use crate::tokens::{color, motion};
use crate::util::{
    cg_color, cg_rect, ease_scalar, layer_host_view, make_panel, rect, spring_scalar, with_completion, without_actions,
    PanelSpec,
};

/// Ring radius in points at full size.
const RADIUS: f64 = 22.0;

/// A mouse-down seen by [`ClickMonitor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClickInfo {
    /// `NSEvent.timestamp`: host-clock seconds since boot.
    pub timestamp: f64,
    /// AppKit global points (bottom-left origin).
    pub point: Point,
    pub button: MouseButton,
    pub click_count: u32,
    /// Raw `NSEventModifierFlags`.
    pub modifier_bits: u64,
}

/// Global + local mouse-down monitors; removed on drop.
pub struct ClickMonitor {
    monitors: Vec<Retained<AnyObject>>,
}

impl std::fmt::Debug for ClickMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClickMonitor")
    }
}

fn info(event: &NSEvent) -> ClickInfo {
    let button = match event.r#type() {
        NSEventType::LeftMouseDown => MouseButton::Left,
        NSEventType::RightMouseDown => MouseButton::Right,
        _ => MouseButton::Other,
    };
    // Control-click is a right click as far as the user is concerned.
    let bits = event.modifierFlags().0 as u64;
    let button = if button == MouseButton::Left && bits & (1 << 18) != 0 { MouseButton::Right } else { button };
    let p = NSEvent::mouseLocation();
    ClickInfo {
        timestamp: event.timestamp(),
        point: Point::new(p.x, p.y),
        button,
        click_count: event.clickCount().max(1) as u32,
        modifier_bits: bits,
    }
}

impl ClickMonitor {
    /// Start monitoring; `handler` runs on the main thread for every
    /// left/right/other mouse-down anywhere on screen.
    pub fn start(_mtm: MainThreadMarker, handler: impl Fn(ClickInfo) + 'static) -> ClickMonitor {
        let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
        let handler: Rc<dyn Fn(ClickInfo)> = Rc::new(handler);
        let mut monitors = Vec::new();
        let h = handler.clone();
        let global = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: the event is valid for the duration of the callback.
            h(info(unsafe { event.as_ref() }));
        });
        if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global) {
            monitors.push(m);
        }
        let local = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: as above; returning the event passes it on unchanged.
            handler(info(unsafe { event.as_ref() }));
            event.as_ptr()
        });
        // SAFETY: the block returns the (valid) event it was given.
        if let Some(m) = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) } {
            monitors.push(m);
        }
        ClickMonitor { monitors }
    }
}

impl Drop for ClickMonitor {
    fn drop(&mut self) {
        for m in &self.monitors {
            // SAFETY: each object came from addGlobal/LocalMonitor….
            unsafe { NSEvent::removeMonitor(m) };
        }
    }
}

struct ScreenPanel {
    /// AppKit global frame.
    frame: Rect,
    panel: Retained<NSPanel>,
    root: Retained<CALayer>,
    scale: f64,
}

/// Transparent ripple panels, one per display.
pub struct ClickOverlay {
    screens: Vec<ScreenPanel>,
}

impl std::fmt::Debug for ClickOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClickOverlay").field("screens", &self.screens.len()).finish()
    }
}

impl ClickOverlay {
    /// Put an (empty) overlay on every screen. Must be on screen before the
    /// recorder fetches shareable content so its windows can be allowed.
    pub fn show(mtm: MainThreadMarker) -> ClickOverlay {
        let screens = NSScreen::screens(mtm)
            .iter()
            .map(|screen| {
                let frame = rect(screen.frame());
                let panel = make_panel(
                    mtm,
                    cg_rect(frame),
                    // Above the recording border so rings sit over everything.
                    PanelSpec {
                        level: NSScreenSaverWindowLevel + 1,
                        keyable: false,
                        shadow: false,
                        ignores_mouse: true,
                    },
                );
                let (view, root) = layer_host_view(mtm, cg_rect(Rect::new(0.0, 0.0, frame.width(), frame.height())));
                panel.setContentView(Some(&view));
                panel.orderFrontRegardless();
                ScreenPanel { frame, panel, root, scale: screen.backingScaleFactor() }
            })
            .collect();
        ClickOverlay { screens }
    }

    /// Window numbers (`CGWindowID`) of the overlay panels.
    pub fn window_numbers(&self) -> Vec<u32> {
        self.screens.iter().map(|s| s.panel.windowNumber().max(0) as u32).collect()
    }

    /// Draw a ripple at `p` (AppKit global points).
    pub fn ripple(&self, p: Point, button: MouseButton) {
        let Some(s) = self.screens.iter().find(|s| s.frame.contains(p)) else { return };
        let tint = match button {
            MouseButton::Left => color::CLICK_LEFT,
            MouseButton::Right => color::CLICK_RIGHT,
            MouseButton::Other => color::CLICK_OTHER,
        };
        // The hosting view and its root layer are top-down (see
        // `layer_host_view`): local y grows downwards from the screen's top.
        let l = lumora_core::geometry::appkit_to_local_top_down(p, s.frame);
        let local = CGPoint::new(l.x, l.y);
        let ring = CAShapeLayer::new();
        let d = RADIUS * 2.0;
        without_actions(|| {
            ring.setBounds(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(d, d)));
            ring.setPosition(local);
            ring.setContentsScale(s.scale);
            // SAFETY: a null transform means identity.
            let path = unsafe {
                CGPath::with_ellipse_in_rect(cg_rect(Rect::new(1.5, 1.5, d - 3.0, d - 3.0)), std::ptr::null())
            };
            ring.setPath(Some(&path));
            ring.setFillColor(Some(&cg_color(tint.with_alpha(0.22))));
            ring.setStrokeColor(Some(&cg_color(tint.with_alpha(0.95))));
            ring.setLineWidth(3.0);
            s.root.addSublayer(&ring);
        });
        let done_ring = ring.clone();
        with_completion(
            || {
                spring_scalar(&ring, "transform.scale", 0.35, 1.0, motion::RIPPLE);
                ease_scalar(&ring, "opacity", 1.0, 0.0, motion::RIPPLE_FADE);
            },
            move || done_ring.removeFromSuperlayer(),
        );
    }
}

impl Drop for ClickOverlay {
    fn drop(&mut self) {
        for s in &self.screens {
            s.panel.orderOut(None);
        }
    }
}
