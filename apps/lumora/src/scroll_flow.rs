//! Scrolling capture: choose an area (live overlay), then the user scrolls
//! while Lumora screenshots that area and stitches the frames that moved
//! (`lumora_core::scroll_capture`). Frames follow the user's scroll-wheel /
//! trackpad events inside the area (a global `NSEvent` monitor, see
//! `lumora_ui::scroll_monitor`; `lumora_core::scroll_trigger` decides when),
//! with a timer as fallback for scrolling that sends no wheel events; the
//! wheel deltas go to the stitcher as hints. No synthetic scroll events and
//! no extra permission. Done / ↩ (or the shortcut again) hands the tall
//! image to the normal capture pipeline; Cancel / Esc drops it.
//!
//! The session never ends on its own before the content has scrolled:
//! identical frames, in-place changes (a click that focuses the window, a
//! hover, a caret) and single failed frames are skipped. After scrolling
//! started it stops by itself only at the height cap, after
//! [`LOST_LIMIT`] frames in a row that line up with nothing, or after
//! [`MAX_FRAME_ERRORS`] failed screenshots in a row. Every end is logged at
//! info with its reason.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_capture::content::ShareableContent;
use lumora_capture::screenshot::{self, ShotTarget};
use lumora_capture::CaptureError;
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Msg};
use lumora_core::scroll_capture::{
    Frame, MatchInfo, Push, ScrollHint, ScrollStitcher, Summary, LOST_LIMIT, MAX_HEIGHT,
};
use lumora_core::scroll_trigger::{self, ScrollScheduler, Trigger, WheelDelta, POLL_INTERVAL};
use lumora_core::selection::Target;
use lumora_ui::recording_hud::RecordingBorder;
use lumora_ui::scroll_hud::ScrollControl;
use lumora_ui::scroll_monitor::{ScrollMonitor, WheelInfo};
use lumora_ui::util;
use objc2::rc::Retained;
use objc2_foundation::NSTimer;

use crate::app::{on_main, App, Mode, Notice};

/// Wheel events logged one by one (debug) before only the frames are.
const DEBUG_EVENTS: usize = 40;

/// Seconds after the first content fetch before the one frames use, so the
/// control and border are on screen in it.
const CONTENT_REFETCH: f64 = 0.12;

/// Failed screenshots in a row before the session stops.
const MAX_FRAME_ERRORS: u32 = 3;

/// Done arriving sooner than this after the control appeared is the tail of
/// the selection gesture (a held ↩, a double-click's second up), not the
/// user finishing: ignored.
const DONE_ARMING: f64 = 0.35;

/// What one frame did, reported to the main thread.
struct FrameOutcome {
    push: Push,
    frames: usize,
    scrolled: bool,
    /// How the frame was matched (offset, fixed bands, scores).
    matched: Option<MatchInfo>,
    /// Counts so far.
    summary: Summary,
    /// Pixels per point of the frame.
    scale: f64,
    /// Why the frame was taken.
    trigger: Trigger,
}

/// Restarted / lost frames logged at info (with how they were matched)
/// before the rest go to debug only.
const INFO_FRAMES: usize = 5;

/// State the capture callbacks (any queue) share with the main thread.
struct Shared {
    /// The stitch so far and the frames' pixels per point.
    stitch: Mutex<(ScrollStitcher, f64)>,
    /// A screenshot is in flight; skip ticks until it lands.
    busy: AtomicBool,
    /// Done or cancelled: late frames are ignored.
    stopped: AtomicBool,
    /// Fetched after our chrome is on screen (refetched once), so it is
    /// excluded.
    content: Mutex<Option<Arc<ShareableContent>>>,
}

/// A running scrolling capture (lives in [`Mode::Scrolling`]).
pub struct ScrollSession {
    control: ScrollControl,
    _border: RecordingBorder,
    timer: Retained<NSTimer>,
    shared: Arc<Shared>,
    /// When frames are taken (main thread only).
    scheduler: RefCell<ScrollScheduler>,
    /// Wheel events from other apps; removed with the session.
    _monitor: ScrollMonitor,
    /// The captured area, global top-left points, whole pixels.
    area: Rect,
    /// The same area in AppKit global points (wheel events' space).
    appkit_area: Rect,
    started: std::time::Instant,
    /// Failed screenshots in a row.
    errors: Cell<u32>,
}

impl std::fmt::Debug for ScrollSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollSession").field("area", &self.area).finish_non_exhaustive()
    }
}

impl App {
    /// The menu item / shortcut: choose an area, or finish the running one.
    pub fn toggle_scrolling_capture(self: &Rc<Self>) {
        let scrolling = matches!(*self.mode.borrow(), Mode::Scrolling(_));
        if scrolling {
            self.finish_scrolling_capture("shortcut or menu pressed again");
        } else {
            self.begin_selection(lumora_ui::events::OverlayMode::Scroll, false);
        }
    }

    /// The overlay chose `target`: show the control and start taking frames.
    pub fn begin_scrolling(self: &Rc<Self>, content: &ShareableContent, target: Target) {
        let (wanted, kind) = match target {
            Target::Area(r) => (r, "area"),
            Target::Window(w) => (w.frame, "window"),
            Target::Display(frame) => (frame, "display"),
        };
        // One display per capture (the frames come from one display's stream).
        let Some((area, _)) = content
            .display_for_rect(wanted)
            .and_then(|d| wanted.intersection(&d.frame).map(|a| (a.integral(d.scale), d.scale)))
        else {
            return self.fail(CaptureError::TargetGone("display"));
        };
        let appkit = util::flip(area, self.mtm);
        let border = RecordingBorder::show_with(self.mtm, appkit, lumora_ui::tokens::color::ACCENT);
        let visible = util::screen_under_mouse(self.mtm).map(|s| util::rect(s.visibleFrame())).unwrap_or(appkit);
        let control = ScrollControl::show(self.mtm, visible, appkit, self.sink.clone());
        let shared = Arc::new(Shared {
            stitch: Mutex::new((ScrollStitcher::new(MAX_HEIGHT), 1.0)),
            busy: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            content: Mutex::new(None),
        });
        let weak = Rc::downgrade(self);
        let timer = util::schedule_timer(POLL_INTERVAL, true, move || {
            let Some(app) = weak.upgrade() else { return false };
            app.scroll_tick()
        });
        let weak = Rc::downgrade(self);
        let monitor = ScrollMonitor::start(self.mtm, move |w| {
            if let Some(app) = weak.upgrade() {
                app.on_wheel(w);
            }
        });
        log::info!(
            "scrolling capture started on a {kind}: {area:?}; scroll-wheel monitor {}",
            if monitor.is_active() { "on" } else { "unavailable, timer only" }
        );
        self.set_mode(Mode::Scrolling(Box::new(ScrollSession {
            control,
            _border: border,
            timer,
            shared,
            scheduler: RefCell::new(ScrollScheduler::default()),
            _monitor: monitor,
            area,
            appkit_area: appkit,
            started: std::time::Instant::now(),
            errors: Cell::new(0),
        })));
        // Frames start once content is fetched with our chrome on screen.
        // Captures exclude Lumora as an application, so its windows never
        // show; the snapshot is still refetched once the control and border
        // have been ordered in, so the window-list fallback (Lumora not
        // listed as an application) covers them too.
        self.fetch_scroll_content(true);
    }

    /// Fetch the content frames are taken from. `first`: refetch once more
    /// after a short delay, once the control and border are surely on screen.
    fn fetch_scroll_content(self: &Rc<Self>, first: bool) {
        ShareableContent::fetch(move |res| {
            on_main(move |app| match res {
                Ok(c) => {
                    let mode = app.mode.borrow();
                    let Mode::Scrolling(s) = &*mode else { return };
                    if first {
                        let weak = Rc::downgrade(app);
                        let _ = util::schedule_timer(CONTENT_REFETCH, false, move || {
                            if let Some(app) = weak.upgrade() {
                                if matches!(*app.mode.borrow(), Mode::Scrolling(_)) {
                                    app.fetch_scroll_content(false);
                                }
                            }
                            false
                        });
                        return;
                    }
                    let own = c.own_window_ids();
                    let control = s.control.window_number();
                    log::info!(
                        "scrolling capture: content ready; Lumora excluded as an app: {}; own windows {:?} (control {}{})",
                        c.excludes_own_app(),
                        own,
                        control,
                        if s.control.is_inside_area() { ", inside the area, ignoring the mouse" } else { ", outside the area" },
                    );
                    if let Ok(mut slot) = s.shared.content.lock() {
                        *slot = Some(Arc::new(c));
                    };
                }
                Err(e) => {
                    if app.end_scrolling().is_some() {
                        log::info!("scrolling capture ended: shareable content failed: {e}");
                        app.set_mode(Mode::Idle);
                        app.fail(e);
                    }
                }
            })
        });
    }

    /// A wheel event anywhere: count it if it is over the area.
    fn on_wheel(self: &Rc<Self>, w: WheelInfo) {
        let mode = self.mode.borrow();
        let Mode::Scrolling(s) = &*mode else { return };
        let Ok(mut sched) = s.scheduler.try_borrow_mut() else { return };
        if !scroll_trigger::in_area(&s.appkit_area, w.point) {
            sched.ignored();
            return;
        }
        sched.wheel(s.started.elapsed().as_secs_f64(), w.event);
        if sched.counts().events <= DEBUG_EVENTS {
            log::debug!(
                "scrolling capture: wheel Δy {:.1} {}{}",
                w.event.delta_y,
                if w.event.precise { "pt" } else { "lines" },
                if w.event.momentum_end {
                    " (momentum end)"
                } else if w.event.momentum {
                    " (momentum)"
                } else {
                    ""
                }
            );
        }
    }

    /// Take the next frame if the scheduler wants one and none is in
    /// flight. `false` stops the timer (the session ended).
    fn scroll_tick(self: &Rc<Self>) -> bool {
        let mode = self.mode.borrow();
        let Mode::Scrolling(s) = &*mode else { return false };
        let content = match s.shared.content.lock() {
            Ok(c) => c.clone(),
            Err(_) => return false,
        };
        let Some(content) = content else { return true };
        if s.shared.busy.load(Ordering::Acquire) {
            return true;
        }
        let now = s.started.elapsed().as_secs_f64();
        let Ok(mut sched) = s.scheduler.try_borrow_mut() else { return true };
        let Some(trigger) = sched.poll(now) else { return true };
        if s.shared.busy.swap(true, Ordering::AcqRel) {
            return true;
        }
        let delta = sched.captured(now, trigger);
        drop(sched);
        if trigger.is_scroll() {
            log::debug!(
                "scrolling capture: frame by {} at {now:.2} s; wheel Δy {:.1} {} over {} event(s)",
                trigger.label(),
                delta.delta_y,
                if delta.precise { "pt" } else { "lines" },
                delta.events
            );
        }
        self.take_scroll_frame(s, content, trigger, delta);
        true
    }

    /// Screenshot the area and stitch it (off the main thread).
    fn take_scroll_frame(
        self: &Rc<Self>,
        s: &ScrollSession,
        content: Arc<ShareableContent>,
        trigger: Trigger,
        delta: WheelDelta,
    ) {
        let shared = s.shared.clone();
        let opts = self.shot_options();
        screenshot::capture(&content, ShotTarget::Area(s.area), opts, move |res| {
            let outcome = match res {
                Ok(shot) if !shared.stopped.load(Ordering::Acquire) => {
                    match lumora_capture::image::bgra_pixels(&shot.image).and_then(|(w, h, px)| Frame::new(w, h, 4, px))
                    {
                        Some(frame) => match shared.stitch.lock() {
                            Ok(mut st) => {
                                st.1 = shot.scale;
                                let hint = ScrollHint::from_wheel(&delta, shot.scale);
                                let push = st.0.push_hinted(frame, hint);
                                Some(Ok(FrameOutcome {
                                    push,
                                    frames: st.0.frames(),
                                    scrolled: st.0.scrolled(),
                                    matched: st.0.last_match().copied(),
                                    summary: st.0.summary(),
                                    scale: shot.scale,
                                    trigger,
                                }))
                            }
                            Err(_) => None,
                        },
                        None => None,
                    }
                }
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            };
            shared.busy.store(false, Ordering::Release);
            if let Some(outcome) = outcome {
                on_main(move |app| app.on_scroll_frame(outcome));
            }
        });
    }

    fn on_scroll_frame(self: &Rc<Self>, outcome: Result<FrameOutcome, CaptureError>) {
        let outcome = {
            let mode = self.mode.borrow();
            let Mode::Scrolling(s) = &*mode else { return };
            match outcome {
                Ok(o) => {
                    s.errors.set(0);
                    s.control.set_frames(o.frames);
                    // The content moved on a timer frame: scrolling without
                    // wheel events (keys, scroll bar); keep the timer fast.
                    let moved = matches!(o.push, Push::Appended(_) | Push::Prepended(_) | Push::Moved);
                    if moved && o.trigger == Trigger::Fallback {
                        if let Ok(mut sched) = s.scheduler.try_borrow_mut() {
                            sched.moved_without_wheel(s.started.elapsed().as_secs_f64());
                        }
                    }
                    Ok(o)
                }
                Err(e) => {
                    let n = s.errors.get() + 1;
                    s.errors.set(n);
                    let scrolled = s.shared.stitch.lock().map(|st| st.0.scrolled()).unwrap_or(false);
                    Err((e, n, scrolled))
                }
            }
        };
        match outcome {
            Err((e, n, _)) if n < MAX_FRAME_ERRORS => {
                log::info!("scrolling capture: frame failed ({n} in a row), still going: {e}");
            }
            Err((e, n, true)) => {
                log::info!("scrolling capture ended: {n} failed frames in a row, keeping the stitch: {e}");
                self.notify(Notice::Warning(Msg::ToastScrollStopped), Some(e.to_string()));
                self.finish_scrolling_capture("frames kept failing");
            }
            Err((e, n, false)) => {
                // Nothing scrolled yet: don't save a single frame the user
                // never asked for.
                log::info!("scrolling capture ended before any scroll: {n} failed frames in a row: {e}");
                if self.end_scrolling().is_some() {
                    self.set_mode(Mode::Idle);
                    self.fail(e);
                }
            }
            Ok(FrameOutcome { push, frames, scrolled, matched, summary, scale, trigger }) => {
                let matched = matched.map_or_else(|| "first frame".to_string(), |m| m.to_string());
                log::debug!(
                    "scrolling capture: frame {} ({}): {push:?}; {matched}; {}x{} px @{scale}x, {frames} kept",
                    summary.pushed,
                    trigger.label(),
                    summary.width,
                    summary.height
                );
                self.on_scroll_push(push, frames, scrolled, &matched, &summary);
            }
        }
    }

    /// React to what the stitcher did with a frame (logs at info only what
    /// helps a report; every frame is logged at debug).
    fn on_scroll_push(self: &Rc<Self>, push: Push, frames: usize, scrolled: bool, matched: &str, summary: &Summary) {
        match push {
            Push::Capped(_) if scrolled => {
                self.notify(Notice::Warning(Msg::ToastScrollCapped), None);
                self.finish_scrolling_capture("height cap reached");
            }
            Push::Capped(_) => {
                log::info!("scrolling capture: the area alone reaches the height cap; waiting for Done");
            }
            Push::NoOverlap => {
                self.notify(
                    Notice::Warning(Msg::ToastScrollNoOverlap),
                    Some(tr(Msg::ToastScrollNoOverlapDetail).into()),
                );
                self.finish_scrolling_capture(&format!("{LOST_LIMIT} frames in a row lined up with nothing"));
            }
            Push::Restarted if summary.restarted <= INFO_FRAMES => {
                log::info!(
                    "scrolling capture: content changed before any scroll, not as a scroll; restarted ({matched})"
                );
            }
            Push::Lost(n) if summary.lost <= INFO_FRAMES => {
                log::info!("scrolling capture: frame lined up with nothing ({n} in a row; {matched})");
            }
            Push::Mismatch => log::info!("scrolling capture: frame of another size skipped"),
            Push::Appended(rows) => log::debug!("scrolling capture: +{rows} rows below ({frames} frames)"),
            Push::Prepended(rows) => log::debug!("scrolling capture: +{rows} rows above ({frames} frames)"),
            Push::Restarted | Push::Lost(_) | Push::Started | Push::Unchanged | Push::Moved => {}
        }
    }

    /// Done / ↩ on the control. Ignored in the first [`DONE_ARMING`]
    /// seconds (the tail of the selection gesture).
    pub fn scroll_done_pressed(self: &Rc<Self>) {
        let age = match &*self.mode.borrow() {
            Mode::Scrolling(s) => s.started.elapsed().as_secs_f64(),
            _ => return,
        };
        if age < DONE_ARMING {
            log::info!("scrolling capture: Done {age:.2} s after the control appeared; ignored (selection gesture)");
            return;
        }
        self.finish_scrolling_capture("Done or ↩ pressed");
    }

    /// Stop taking frames and close the chrome; returns the session.
    fn end_scrolling(self: &Rc<Self>) -> Option<Box<ScrollSession>> {
        if !matches!(*self.mode.borrow(), Mode::Scrolling(_)) {
            return None;
        }
        let Mode::Scrolling(s) = self.mode.replace(Mode::Preparing) else { return None };
        s.shared.stopped.store(true, Ordering::Release);
        s.timer.invalidate();
        Some(s)
    }

    /// Stitch what we have and send it down the capture pipeline. `reason`
    /// is logged.
    pub fn finish_scrolling_capture(self: &Rc<Self>, reason: &str) {
        let Some(session) = self.end_scrolling() else { return };
        let ScrollSession { shared, area, started, scheduler, .. } = *session;
        let triggers = scheduler.into_inner().counts();
        let (frames, height, summary, last) = shared
            .stitch
            .lock()
            .map(|st| (st.0.frames(), st.0.height(), st.0.summary(), st.0.last_match().copied()))
            .unwrap_or_default();
        log::info!(
            "scrolling capture finished after {:.1} s: {reason} ({frames} frame(s), {height} px)",
            started.elapsed().as_secs_f64()
        );
        log::info!(
            "scrolling capture summary: {summary}; last match: {}",
            last.map_or_else(|| "none".to_string(), |m| m.to_string())
        );
        log::info!("scrolling capture frames: {triggers}");
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                // Waits for a frame still being stitched.
                let taken = shared
                    .stitch
                    .lock()
                    .map(|mut st| (std::mem::replace(&mut st.0, ScrollStitcher::new(1)), st.1))
                    .ok();
                let result = match taken {
                    Some((st, scale)) if st.frames() > 0 => {
                        let (w, h, px) = st.into_image();
                        lumora_capture::image::from_bgra(w, h, &px).map(|img| (img, scale, w, h))
                    }
                    _ => Err(CaptureError::Empty),
                };
                on_main(move |app| {
                    app.set_mode(Mode::Idle);
                    match result {
                        Ok((img, scale, w, h)) => {
                            let s = if scale > 0.0 { scale } else { 1.0 };
                            let frame = Rect::new(area.min_x(), area.min_y(), w as f64 / s, h as f64 / s);
                            app.finish_capture(img, s, frame);
                        }
                        Err(e) => app.fail(e),
                    }
                });
            },
        );
    }

    /// Cancel / Esc: throw the frames away.
    pub fn cancel_scrolling_capture(self: &Rc<Self>) {
        if self.end_scrolling().is_some() {
            log::info!("scrolling capture cancelled (Cancel or Esc)");
            self.set_mode(Mode::Idle);
        }
    }
}
