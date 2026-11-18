//! Scrolling capture: choose an area (live overlay), then the user scrolls
//! while Lumora screenshots that area every [`TICK`] seconds and stitches
//! the frames that moved (`lumora_core::scroll_capture`). No synthetic
//! scroll events, so no Accessibility permission. Done / ↩ (or the shortcut
//! again) hands the tall image to the normal capture pipeline; Cancel / Esc
//! drops it.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_capture::content::ShareableContent;
use lumora_capture::screenshot::{self, ShotTarget};
use lumora_capture::CaptureError;
use lumora_core::geometry::Rect;
use lumora_core::i18n::{tr, Msg};
use lumora_core::scroll_capture::{Frame, Push, ScrollStitcher, MAX_HEIGHT};
use lumora_core::selection::Target;
use lumora_ui::recording_hud::RecordingBorder;
use lumora_ui::scroll_hud::ScrollControl;
use lumora_ui::util;
use objc2::rc::Retained;
use objc2_foundation::NSTimer;

use crate::app::{on_main, App, Mode, Notice};

/// Seconds between frames.
const TICK: f64 = 0.15;

/// State the capture callbacks (any queue) share with the main thread.
struct Shared {
    /// The stitch so far and the frames' pixels per point.
    stitch: Mutex<(ScrollStitcher, f64)>,
    /// A screenshot is in flight; skip ticks until it lands.
    busy: AtomicBool,
    /// Done or cancelled: late frames are ignored.
    stopped: AtomicBool,
    /// Fetched after our chrome is on screen, so it is excluded.
    content: Mutex<Option<Arc<ShareableContent>>>,
}

/// A running scrolling capture (lives in [`Mode::Scrolling`]).
pub struct ScrollSession {
    control: ScrollControl,
    _border: RecordingBorder,
    timer: Retained<NSTimer>,
    shared: Arc<Shared>,
    /// The captured area, global top-left points, whole pixels.
    area: Rect,
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
            self.finish_scrolling_capture();
        } else {
            self.begin_selection(lumora_ui::events::OverlayMode::Scroll, false);
        }
    }

    /// The overlay chose `target`: show the control and start taking frames.
    pub fn begin_scrolling(self: &Rc<Self>, content: &ShareableContent, target: Target) {
        let wanted = match target {
            Target::Area(r) => r,
            Target::Window(w) => w.frame,
            Target::Display(frame) => frame,
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
        let timer = util::schedule_timer(TICK, true, move || {
            let Some(app) = weak.upgrade() else { return false };
            app.scroll_tick()
        });
        self.set_mode(Mode::Scrolling(Box::new(ScrollSession { control, _border: border, timer, shared, area })));
        // Fetch content *after* our chrome is on screen so it gets excluded.
        ShareableContent::fetch(move |res| {
            on_main(move |app| match res {
                Ok(c) => {
                    if let Mode::Scrolling(s) = &*app.mode.borrow() {
                        if let Ok(mut slot) = s.shared.content.lock() {
                            *slot = Some(Arc::new(c));
                        }
                    }
                }
                Err(e) => {
                    app.end_scrolling();
                    app.fail(e);
                }
            })
        });
    }

    /// Take the next frame unless one is still in flight. `false` stops the
    /// timer (the session ended).
    fn scroll_tick(self: &Rc<Self>) -> bool {
        let mode = self.mode.borrow();
        let Mode::Scrolling(s) = &*mode else { return false };
        let content = match s.shared.content.lock() {
            Ok(c) => c.clone(),
            Err(_) => return false,
        };
        let Some(content) = content else { return true };
        if s.shared.busy.swap(true, Ordering::AcqRel) {
            return true;
        }
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
                                let r = st.0.push(frame);
                                Some(Ok((r, st.0.frames())))
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
        true
    }

    fn on_scroll_frame(self: &Rc<Self>, outcome: Result<(Push, usize), CaptureError>) {
        let (push, frames) = match outcome {
            Ok(o) => o,
            Err(e) => {
                // A frame failed (display gone, permission revoked): keep
                // what is stitched.
                log::warn!("scrolling capture frame failed: {e}");
                return self.finish_scrolling_capture();
            }
        };
        if let Mode::Scrolling(s) = &*self.mode.borrow() {
            s.control.set_frames(frames);
        } else {
            return;
        }
        match push {
            Push::Capped(_) => {
                self.notify(Notice::Warning(Msg::ToastScrollCapped), None);
                self.finish_scrolling_capture();
            }
            Push::NoOverlap => {
                self.notify(
                    Notice::Warning(Msg::ToastScrollNoOverlap),
                    Some(tr(Msg::ToastScrollNoOverlapDetail).into()),
                );
                self.finish_scrolling_capture();
            }
            _ => {}
        }
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

    /// Done / ↩: stitch what we have and send it down the capture pipeline.
    pub fn finish_scrolling_capture(self: &Rc<Self>) {
        let Some(session) = self.end_scrolling() else { return };
        let ScrollSession { shared, area, .. } = *session;
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
            self.set_mode(Mode::Idle);
        }
    }
}
