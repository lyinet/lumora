//! Screen recording flow:
//! overlay (Record mode) → border → countdown → stop control + recorder →
//! menu-bar timer → stop → finalise → Quick Access card.

use std::rc::Rc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_capture::content::ShareableContent;
use lumora_capture::recorder::{self, RecordOptions, Recorder, RecorderEvent};
use lumora_capture::CaptureError;
use lumora_core::filename::{render, unique_path, CaptureKind};
use lumora_core::geometry::{Rect, Size};
use lumora_core::recording::{Event, Phase};
use lumora_core::selection::Target;
use lumora_ui::countdown::Countdown;
use lumora_ui::quick_access::CardContent;
use lumora_ui::recording_hud::{RecordingBorder, StopControl};
use lumora_ui::util;

use lumora_core::i18n::Msg;

use crate::app::{on_main, App, CaptureRecord, Mode, Notice, RecordTarget};
use crate::system;

impl App {
    pub fn toggle_recording(self: &Rc<Self>) {
        let state = match &*self.mode.borrow() {
            Mode::Recording { .. } => 1,
            Mode::Countdown { .. } => 2,
            Mode::Idle => 0,
            _ => 3,
        };
        match state {
            0 => self.begin_selection(lumora_ui::events::OverlayMode::Record, false),
            1 => self.stop_recording(),
            2 => self.cancel_recording_setup(),
            _ => {}
        }
    }

    /// Resolve the overlay's choice into a display + optional area.
    pub fn on_record_target(self: &Rc<Self>, content: &ShareableContent, target: Target) {
        // TODO(multi-display-recording): one SCStream records one display, so
        // an area spanning displays is clipped to the display it mostly
        // covers (screenshots stitch; recordings would need two streams).
        let resolved = match target {
            Target::Area(r) => content
                .display_for_rect(r)
                .and_then(|d| r.intersection(&d.frame).map(|a| (d, Some(a.integral(d.scale))))),
            Target::Window(w) => content
                .display_for_rect(w.frame)
                .and_then(|d| w.frame.intersection(&d.frame).map(|a| (d, Some(a.integral(d.scale))))),
            Target::Display(frame) => content.displays().into_iter().find(|d| d.frame == frame).map(|d| (d, None)),
        };
        let Some((display, area)) = resolved else { return self.fail(CaptureError::TargetGone("display")) };
        let appkit_frame = util::flip(area.unwrap_or(display.frame), self.mtm);
        self.prepare_recording(RecordTarget { display_id: display.id, area, appkit_frame });
    }

    fn prepare_recording(self: &Rc<Self>, target: RecordTarget) {
        let border = target.area.map(|_| RecordingBorder::show(self.mtm, target.appkit_frame));
        let secs = self.settings.borrow().recording.countdown;
        let _ = self.session.borrow_mut().apply(Event::Begin { countdown: secs });
        if secs > 0 {
            let countdown = Countdown::start(self.mtm, secs, target.appkit_frame, self.sink.clone());
            self.set_mode(Mode::Countdown { _countdown: countdown, target, border });
        } else {
            self.start_recorder(target, border);
        }
    }

    pub fn on_countdown_finished(self: &Rc<Self>) {
        if !matches!(*self.mode.borrow(), Mode::Countdown { .. }) {
            return;
        }
        let Mode::Countdown { target, border, .. } = self.mode.replace(Mode::Preparing) else { return };
        {
            let mut session = self.session.borrow_mut();
            while matches!(session.phase(), Phase::Countdown { .. }) {
                let _ = session.apply(Event::Tick);
            }
        }
        self.start_recorder(target, border);
    }

    pub fn cancel_recording_setup(self: &Rc<Self>) {
        let _ = self.session.borrow_mut().apply(Event::Cancel);
        self.set_mode(Mode::Idle);
    }

    fn start_recorder(self: &Rc<Self>, target: RecordTarget, border: Option<RecordingBorder>) {
        let visible =
            util::screen_under_mouse(self.mtm).map(|s| util::rect(s.visibleFrame())).unwrap_or(target.appkit_frame);
        let stop = StopControl::show(self.mtm, visible, target.appkit_frame, self.sink.clone());
        let weak = Rc::downgrade(self);
        let timer = util::schedule_timer(0.25, true, move || {
            let Some(app) = weak.upgrade() else { return false };
            let elapsed = app.session.borrow().elapsed(util::now());
            let mode = app.mode.borrow();
            let Mode::Recording { stop, .. } = &*mode else { return false };
            stop.set_elapsed(elapsed);
            app.status.set_recording(Some(elapsed));
            true
        });
        self.status.set_recording(Some(0.0));
        self.set_mode(Mode::Recording { recorder: None, target, _border: border, stop, timer });
        // Fetch content *after* our chrome is on screen so it gets excluded.
        ShareableContent::fetch(move |res| on_main(move |app| app.on_content_for_recording(res)));
    }

    fn on_content_for_recording(self: &Rc<Self>, res: Result<ShareableContent, CaptureError>) {
        let content = match res {
            Ok(c) => c,
            Err(e) => {
                self.status.set_recording(None);
                let _ = self.session.borrow_mut().apply(Event::Cancel);
                return self.fail_with(Msg::ToastRecordingFailed, e);
            }
        };
        let target = match &*self.mode.borrow() {
            Mode::Recording { target, recorder: None, .. } => *target,
            _ => return, // stopped/cancelled meanwhile
        };
        let s = self.settings.borrow().clone();
        let stem = render(&s.general.filename_template, &system::now_parts(), CaptureKind::Recording);
        let dir = if s.general.save_to_disk { s.save_folder(&self.home) } else { system::temp_dir() };
        let _ = std::fs::create_dir_all(&dir);
        let output = unique_path(&dir, &stem, CaptureKind::Recording.extension(), |p| p.exists());
        let opts =
            RecordOptions { display_id: target.display_id, area: target.area, settings: s.recording.clone(), output };
        match Recorder::start(&content, opts, |event| on_main(move |app| app.on_recorder_event(event))) {
            Ok(r) => {
                if let Mode::Recording { recorder, .. } = &mut *self.mode.borrow_mut() {
                    *recorder = Some(r);
                }
            }
            Err(e) => {
                self.status.set_recording(None);
                let _ = self.session.borrow_mut().apply(Event::Cancel);
                self.fail_with(Msg::ToastRecordingFailed, e);
            }
        }
    }

    fn on_recorder_event(self: &Rc<Self>, event: RecorderEvent) {
        match event {
            RecorderEvent::Started => {
                let _ = self.session.borrow_mut().apply(Event::Started { now: util::now() });
            }
            RecorderEvent::Failed(e) => {
                log::error!("recording stopped: {e}");
                self.stop_recording();
                self.notify(Notice::Error(Msg::ToastRecordingFailed), Some(crate::capture_flow::error_text(&e)));
            }
        }
    }

    pub fn stop_recording(self: &Rc<Self>) {
        if !matches!(*self.mode.borrow(), Mode::Recording { .. }) {
            return;
        }
        let Mode::Recording { recorder, target, timer, .. } = self.mode.replace(Mode::Finishing) else { return };
        timer.invalidate();
        self.status.set_recording(None);
        let now = util::now();
        let duration = {
            let mut session = self.session.borrow_mut();
            let _ = session.apply(Event::Stop { now });
            session.elapsed(now)
        };
        match recorder {
            Some(r) => r.stop(move |res| on_main(move |app| app.on_recording_finished(res, target, duration))),
            None => {
                let _ = self.session.borrow_mut().apply(Event::Cancel);
                self.set_mode(Mode::Idle);
            }
        }
    }

    fn on_recording_finished(
        self: &Rc<Self>,
        res: Result<std::path::PathBuf, CaptureError>,
        target: RecordTarget,
        duration: f64,
    ) {
        let _ = self.session.borrow_mut().apply(Event::Finished);
        self.set_mode(Mode::Idle);
        let path = match res {
            Ok(p) => p,
            Err(e) => return self.fail(e),
        };
        log::info!("recorded {} ({duration:.1}s)", path.display());
        let s = self.settings.borrow().clone();
        if s.general.copy_to_clipboard {
            lumora_capture::clipboard::copy_file(&path);
        }
        let id = self.next_id();
        let frame: Rect = target.area.unwrap_or_else(|| util::flip(target.appkit_frame, self.mtm));
        let temp = !s.general.save_to_disk;
        let show_card = s.general.show_quick_access;
        // Poster frame off the main thread, then show the card.
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let thumb = recorder::thumbnail(&path, 640.0);
                on_main(move |app| {
                    app.captures.borrow_mut().insert(
                        id,
                        CaptureRecord {
                            image: thumb.clone(),
                            scale: 2.0,
                            frame,
                            file: path.clone(),
                            temp,
                            video: true,
                        },
                    );
                    if show_card {
                        app.quick.push(CardContent {
                            id,
                            image: thumb,
                            point_size: Size::new(frame.width(), frame.height()),
                            file: path,
                            video_duration: Some(duration),
                        });
                    }
                });
            },
        );
    }
}
