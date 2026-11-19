//! Scroll-wheel events anywhere on screen, for scrolling capture.
//!
//! A *global* `NSEvent` monitor for `NSEventMaskScrollWheel`. Apple's docs
//! (`addGlobalMonitorForEventsMatchingMask:handler:`) require Accessibility
//! only for key-related events, so scroll events need no permission beyond
//! the Screen Recording grant scrolling capture already has. A listen-only
//! `CGEventTap` (what the keystroke HUD uses) was not used: taps go through
//! the Input Monitoring (`kTCCServiceListenEvent`) check, which could mean a
//! second prompt for no gain. The monitor only observes; events still reach
//! the app under the pointer.
//!
//! Events sent to Lumora's own windows are not seen (that is how global
//! monitors work); they never scroll the captured app anyway.

use std::ptr::NonNull;

use block2::RcBlock;
use lumora_core::geometry::Point;
use lumora_core::scroll_trigger::WheelEvent;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase};

/// A scroll-wheel event seen by [`ScrollMonitor`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelInfo {
    pub event: WheelEvent,
    /// Pointer position, AppKit global points (bottom-left origin).
    pub point: Point,
    /// `NSEvent.timestamp`: host-clock seconds since boot.
    pub timestamp: f64,
}

/// Global scroll-wheel monitor; removed on drop.
pub struct ScrollMonitor {
    monitor: Option<Retained<AnyObject>>,
}

impl std::fmt::Debug for ScrollMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollMonitor").field("active", &self.monitor.is_some()).finish()
    }
}

fn info(event: &NSEvent) -> WheelInfo {
    let momentum = event.momentumPhase();
    let p = NSEvent::mouseLocation();
    WheelInfo {
        event: WheelEvent {
            delta_y: event.scrollingDeltaY(),
            precise: event.hasPreciseScrollingDeltas(),
            momentum: !momentum.is_empty(),
            momentum_end: momentum.contains(NSEventPhase::Ended) || momentum.contains(NSEventPhase::Cancelled),
        },
        point: Point::new(p.x, p.y),
        timestamp: event.timestamp(),
    }
}

impl ScrollMonitor {
    /// Start monitoring; `handler` runs on the main thread for every
    /// scroll-wheel event sent to another app.
    pub fn start(_mtm: MainThreadMarker, handler: impl Fn(WheelInfo) + 'static) -> ScrollMonitor {
        let block = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: the event is valid for the duration of the callback.
            handler(info(unsafe { event.as_ref() }));
        });
        let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(NSEventMask::ScrollWheel, &block);
        ScrollMonitor { monitor }
    }

    /// The system accepted the monitor.
    pub fn is_active(&self) -> bool {
        self.monitor.is_some()
    }
}

impl Drop for ScrollMonitor {
    fn drop(&mut self) {
        if let Some(m) = &self.monitor {
            // SAFETY: the object came from addGlobalMonitor….
            unsafe { NSEvent::removeMonitor(m) };
        }
    }
}
