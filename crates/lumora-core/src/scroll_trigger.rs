//! When a scrolling capture takes its next frame.
//!
//! Frames are driven by the user's scroll-wheel / trackpad events inside the
//! captured area instead of a fixed timer:
//!
//! * **Continuous** — while events keep arriving, a frame at most every
//!   [`Timing::max_interval`] (the first event after a pause captures as
//!   soon as that much time has passed since the previous frame).
//! * **Settled** — [`Timing::debounce`] after the last event, one frame of
//!   where the content came to rest.
//! * **Momentum end** — the event that ends a trackpad fling captures after
//!   a short [`Timing::settle`], replacing the settled frame.
//! * **Fallback** — a timer for scrolling that sends no wheel events (keys,
//!   dragging the scroll bar, an auto-scrolling page). Until the session has
//!   seen a wheel event it runs at the old fixed tick
//!   ([`Timing::fast_fallback`]), so those sessions behave as before; after
//!   that it slows to [`Timing::slow_fallback`], and speeds up again for
//!   [`Timing::fast_window`] whenever a fallback frame shows that the content
//!   moved without any wheel event.
//!
//! The scheduler also sums the events' deltas since the last frame into a
//! [`WheelDelta`], which the stitcher turns into a hint about where the
//! content went ([`crate::scroll_capture::ScrollHint`]).
//!
//! Pure state machine: times are seconds on any monotonic clock, passed in.
//! The platform layer polls [`ScrollScheduler::poll`] from a short timer
//! while no frame is in flight and calls [`ScrollScheduler::captured`] when it
//! starts one.

use std::fmt;

use crate::geometry::{Point, Rect};

/// The scheduler's intervals, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// Quiet time after the last event before the settled frame.
    pub debounce: f64,
    /// Shortest gap between frames while events keep arriving.
    pub max_interval: f64,
    /// Delay after the momentum-end event (lets the app draw the last step).
    pub settle: f64,
    /// Timer before any wheel event was seen (the v0.6 fixed tick).
    pub fast_fallback: f64,
    /// Timer once wheel events drive the session.
    pub slow_fallback: f64,
    /// How long the fast timer comes back after movement without events.
    pub fast_window: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            debounce: 0.07,
            max_interval: 0.12,
            settle: 0.03,
            fast_fallback: 0.15,
            slow_fallback: 0.4,
            fast_window: 1.5,
        }
    }
}

/// How often the platform should call [`ScrollScheduler::poll`], in seconds.
pub const POLL_INTERVAL: f64 = 0.02;

/// One scroll-wheel / trackpad event.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WheelEvent {
    /// `NSEvent.scrollingDeltaY`: positive when the content moves down
    /// (towards its top). Points when `precise`, else lines.
    pub delta_y: f64,
    /// `hasPreciseScrollingDeltas` (trackpad, Magic Mouse).
    pub precise: bool,
    /// Part of the momentum (fling) after the fingers left the trackpad.
    pub momentum: bool,
    /// The momentum ended with this event.
    pub momentum_end: bool,
}

/// Wheel deltas summed since the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WheelDelta {
    /// Sum of `delta_y` (see [`WheelEvent::delta_y`] for the sign).
    pub delta_y: f64,
    /// Every summed event was precise (points, not lines).
    pub precise: bool,
    /// Events summed.
    pub events: usize,
}

impl WheelDelta {
    fn add(&mut self, e: &WheelEvent) {
        if self.events == 0 {
            self.precise = e.precise;
        } else {
            self.precise &= e.precise;
        }
        self.delta_y += e.delta_y;
        self.events += 1;
    }
}

/// Why a frame was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// Events are still arriving.
    Continuous,
    /// The events stopped [`Timing::debounce`] ago.
    Settled,
    /// The momentum ended.
    MomentumEnd,
    /// Timer (no pending wheel events).
    Fallback,
}

impl Trigger {
    /// Taken because of wheel events.
    pub fn is_scroll(self) -> bool {
        !matches!(self, Trigger::Fallback)
    }

    pub fn label(self) -> &'static str {
        match self {
            Trigger::Continuous => "scroll",
            Trigger::Settled => "scroll settled",
            Trigger::MomentumEnd => "momentum end",
            Trigger::Fallback => "timer",
        }
    }
}

/// Counts for the end-of-session log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TriggerCounts {
    pub continuous: usize,
    pub settled: usize,
    pub momentum_end: usize,
    pub fallback: usize,
    /// Wheel events inside the area.
    pub events: usize,
    /// Wheel events outside the area (ignored).
    pub ignored: usize,
}

impl TriggerCounts {
    /// Frames taken because of wheel events.
    pub fn by_scroll(&self) -> usize {
        self.continuous + self.settled + self.momentum_end
    }
}

impl fmt::Display for TriggerCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} frame(s) by scroll ({} continuous, {} settled, {} momentum end), {} by timer; \
             {} wheel event(s) in the area, {} outside ignored",
            self.by_scroll(),
            self.continuous,
            self.settled,
            self.momentum_end,
            self.fallback,
            self.events,
            self.ignored
        )
    }
}

/// Whether a wheel event at `point` concerns the capture: inside `area`
/// (same coordinate space, edges included).
pub fn in_area(area: &Rect, point: Point) -> bool {
    point.x >= area.min_x() && point.x <= area.max_x() && point.y >= area.min_y() && point.y <= area.max_y()
}

/// Decides when the next frame is taken.
#[derive(Debug, Clone)]
pub struct ScrollScheduler {
    timing: Timing,
    /// Time of the last frame started (`None`: none yet).
    last_capture: Option<f64>,
    /// Time of the last wheel event.
    last_event: Option<f64>,
    /// Wheel events arrived since the last frame.
    pending: bool,
    /// Wheel events arrived since the last settled / momentum-end frame.
    unsettled: bool,
    /// When the momentum-end frame is due.
    momentum_end_at: Option<f64>,
    /// A wheel event was seen in this session.
    seen_wheel: bool,
    /// Fast fallback until this time (movement without wheel events).
    fast_until: Option<f64>,
    delta: WheelDelta,
    counts: TriggerCounts,
}

impl Default for ScrollScheduler {
    fn default() -> Self {
        ScrollScheduler::new(Timing::default())
    }
}

impl ScrollScheduler {
    pub fn new(timing: Timing) -> Self {
        ScrollScheduler {
            timing,
            last_capture: None,
            last_event: None,
            pending: false,
            unsettled: false,
            momentum_end_at: None,
            seen_wheel: false,
            fast_until: None,
            delta: WheelDelta::default(),
            counts: TriggerCounts::default(),
        }
    }

    pub fn timing(&self) -> Timing {
        self.timing
    }

    /// A wheel event inside the area at `now`.
    pub fn wheel(&mut self, now: f64, e: WheelEvent) {
        self.counts.events += 1;
        self.seen_wheel = true;
        self.last_event = Some(now);
        self.pending = true;
        self.unsettled = true;
        self.delta.add(&e);
        if e.momentum_end {
            self.momentum_end_at = Some(now + self.timing.settle);
        } else if e.momentum {
            // Momentum resumed (a second fling): the end comes later.
            self.momentum_end_at = None;
        }
    }

    /// A wheel event outside the area (counted, otherwise ignored).
    pub fn ignored(&mut self) {
        self.counts.ignored += 1;
    }

    /// A fallback frame showed the content moved without wheel events
    /// (keys, scroll bar): use the fast timer for a while.
    pub fn moved_without_wheel(&mut self, now: f64) {
        self.fast_until = Some(now + self.timing.fast_window);
    }

    fn fallback_interval(&self, now: f64) -> f64 {
        let fast = !self.seen_wheel || self.fast_until.is_some_and(|t| now < t);
        if fast {
            self.timing.fast_fallback
        } else {
            self.timing.slow_fallback
        }
    }

    /// The frame to take at `now`, if any. Call only while no frame is in
    /// flight; a returned trigger must be followed by [`Self::captured`].
    pub fn poll(&self, now: f64) -> Option<Trigger> {
        let since_capture = self.last_capture.map_or(f64::INFINITY, |t| now - t);
        if let Some(at) = self.momentum_end_at {
            if now >= at {
                return Some(Trigger::MomentumEnd);
            }
        }
        if self.unsettled {
            let quiet = self.last_event.map_or(f64::INFINITY, |t| now - t);
            if quiet >= self.timing.debounce {
                return Some(Trigger::Settled);
            }
            if self.pending && since_capture >= self.timing.max_interval {
                return Some(Trigger::Continuous);
            }
            // Events are arriving; the next one or the debounce decides.
            return None;
        }
        (since_capture >= self.fallback_interval(now)).then_some(Trigger::Fallback)
    }

    /// A frame for `trigger` started at `now`; returns the wheel deltas
    /// summed since the previous frame (the hint for this one).
    pub fn captured(&mut self, now: f64, trigger: Trigger) -> WheelDelta {
        self.last_capture = Some(now);
        self.pending = false;
        if matches!(trigger, Trigger::Settled | Trigger::MomentumEnd) {
            self.unsettled = false;
            self.momentum_end_at = None;
        }
        match trigger {
            Trigger::Continuous => self.counts.continuous += 1,
            Trigger::Settled => self.counts.settled += 1,
            Trigger::MomentumEnd => self.counts.momentum_end += 1,
            Trigger::Fallback => self.counts.fallback += 1,
        }
        std::mem::take(&mut self.delta)
    }

    pub fn counts(&self) -> TriggerCounts {
        self.counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(dy: f64) -> WheelEvent {
        WheelEvent { delta_y: dy, precise: true, ..Default::default() }
    }

    /// Run the scheduler like the platform does: poll every 10 ms, a frame
    /// takes `busy` seconds; `events` are (time, event). Returns the frames.
    fn run(s: &mut ScrollScheduler, events: &[(f64, WheelEvent)], until: f64, busy: f64) -> Vec<(f64, Trigger)> {
        let mut out = Vec::new();
        let mut i = 0;
        let mut free_at = 0.0;
        let mut k = 0u32;
        loop {
            let t = f64::from(k) * 0.01;
            if t > until + 1e-9 {
                break;
            }
            while i < events.len() && events[i].0 <= t + 1e-9 {
                s.wheel(events[i].0, events[i].1);
                i += 1;
            }
            if t + 1e-9 >= free_at {
                if let Some(trig) = s.poll(t) {
                    s.captured(t, trig);
                    out.push((t, trig));
                    free_at = t + busy;
                }
            }
            k += 1;
        }
        out
    }

    #[test]
    fn first_frame_and_fast_timer_before_any_wheel_event() {
        let mut s = ScrollScheduler::default();
        assert_eq!(s.poll(0.0), Some(Trigger::Fallback));
        s.captured(0.0, Trigger::Fallback);
        assert_eq!(s.poll(0.1), None);
        // No wheel event yet: the v0.6 tick, so keyboard scrolling is unchanged.
        assert_eq!(s.poll(0.15), Some(Trigger::Fallback));
    }

    #[test]
    fn a_single_notch_captures_at_once_then_settles_once() {
        let mut s = ScrollScheduler::default();
        s.captured(0.0, Trigger::Fallback);
        let frames = run(&mut s, &[(1.0, ev(-10.0))], 1.3, 0.03);
        let scroll: Vec<_> = frames.iter().filter(|f| f.1.is_scroll()).collect();
        assert_eq!(scroll.len(), 2, "{frames:?}");
        assert_eq!(scroll[0].1, Trigger::Continuous);
        assert!(scroll[0].0 >= 1.0 - 1e-9 && scroll[0].0 <= 1.03, "{:?}", scroll[0]);
        assert_eq!(scroll[1].1, Trigger::Settled);
        assert!((scroll[1].0 - 1.07).abs() < 0.011, "{:?}", scroll[1]);
    }

    #[test]
    fn continuous_scrolling_is_throttled_to_the_max_interval() {
        let mut s = ScrollScheduler::default();
        s.captured(0.0, Trigger::Fallback);
        // An event every 16 ms for one second.
        let events: Vec<_> = (0..63).map(|i| (0.5 + i as f64 * 0.016, ev(-4.0))).collect();
        let frames = run(&mut s, &events, 1.6, 0.03);
        let during: Vec<_> = frames.iter().filter(|f| f.0 >= 0.5 && f.0 <= 1.5).collect();
        assert!(during.iter().all(|f| f.1 == Trigger::Continuous));
        // ~1 s / 120 ms
        assert!((8..=10).contains(&during.len()), "{during:?}");
        for w in during.windows(2) {
            assert!(w[1].0 - w[0].0 >= 0.12 - 1e-6);
        }
        // Then exactly one settled frame.
        let settled = frames.iter().filter(|f| f.1 == Trigger::Settled).count();
        assert_eq!(settled, 1);
    }

    #[test]
    fn momentum_end_replaces_the_settled_frame() {
        let mut s = ScrollScheduler::default();
        s.captured(0.0, Trigger::Fallback);
        let mut events: Vec<_> = (0..10).map(|i| (0.2 + i as f64 * 0.016, ev(-8.0))).collect();
        let m = |dy, end| WheelEvent { delta_y: dy, precise: true, momentum: true, momentum_end: end };
        events.extend((0..10).map(|i| (0.36 + i as f64 * 0.016, m(-3.0, false))));
        events.push((0.53, m(0.0, true)));
        let frames = run(&mut s, &events, 1.0, 0.02);
        let ends: Vec<_> = frames.iter().filter(|f| f.1 == Trigger::MomentumEnd).collect();
        assert_eq!(ends.len(), 1, "{frames:?}");
        assert!((ends[0].0 - 0.56).abs() < 0.011);
        assert_eq!(frames.iter().filter(|f| f.1 == Trigger::Settled).count(), 0, "{frames:?}");
    }

    #[test]
    fn a_frame_in_flight_delays_but_never_drops_the_settled_frame() {
        let mut s = ScrollScheduler::default();
        s.captured(0.0, Trigger::Fallback);
        // Slow screenshots (200 ms).
        let events: Vec<_> = (0..20).map(|i| (0.3 + i as f64 * 0.016, ev(-5.0))).collect();
        let frames = run(&mut s, &events, 1.5, 0.2);
        let last_event = events.last().unwrap().0;
        assert!(frames.iter().any(|f| f.0 > last_event && f.1 == Trigger::Settled), "{frames:?}");
    }

    #[test]
    fn slow_fallback_after_wheel_events_and_fast_again_on_movement_without_them() {
        let mut s = ScrollScheduler::default();
        s.wheel(0.0, ev(-3.0));
        s.captured(0.1, Trigger::Settled);
        assert_eq!(s.poll(0.3), None);
        assert_eq!(s.poll(0.55), Some(Trigger::Fallback));
        s.captured(0.55, Trigger::Fallback);
        s.moved_without_wheel(0.57);
        assert_eq!(s.poll(0.71), Some(Trigger::Fallback));
        s.captured(0.71, Trigger::Fallback);
        // The fast window ends.
        s.captured(2.1, Trigger::Fallback);
        assert_eq!(s.poll(2.3), None);
        assert_eq!(s.poll(2.55), Some(Trigger::Fallback));
    }

    #[test]
    fn deltas_are_summed_per_frame() {
        let mut s = ScrollScheduler::default();
        s.wheel(0.0, ev(-10.0));
        s.wheel(0.01, ev(-5.5));
        let d = s.captured(0.02, Trigger::Continuous);
        assert_eq!(d, WheelDelta { delta_y: -15.5, precise: true, events: 2 });
        assert_eq!(s.captured(0.5, Trigger::Fallback), WheelDelta::default());
        s.wheel(1.0, ev(-1.0));
        s.wheel(1.01, WheelEvent { delta_y: -1.0, precise: false, ..Default::default() });
        let d = s.captured(1.1, Trigger::Settled);
        assert!(!d.precise);
        assert_eq!(d.events, 2);
    }

    #[test]
    fn counts_and_area() {
        let mut s = ScrollScheduler::default();
        s.wheel(0.0, ev(-1.0));
        s.ignored();
        s.captured(0.0, Trigger::Continuous);
        s.captured(0.1, Trigger::Settled);
        s.captured(0.5, Trigger::Fallback);
        let c = s.counts();
        assert_eq!((c.by_scroll(), c.fallback, c.events, c.ignored), (2, 1, 1, 1));
        assert!(c.to_string().starts_with("2 frame(s) by scroll"));
        let area = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(in_area(&area, Point::new(10.0, 20.0)));
        assert!(in_area(&area, Point::new(110.0, 70.0)));
        assert!(!in_area(&area, Point::new(9.9, 30.0)));
        assert!(!in_area(&area, Point::new(50.0, 70.1)));
    }
}
