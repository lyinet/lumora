//! Screen recording flow:
//! overlay (Record mode) → border → countdown → stop control + recorder →
//! menu-bar timer → stop → finalise → Quick Access card.
//!
//! While recording, every mouse-down is (a) drawn as a ripple on the
//! click overlay (which, unlike the rest of our chrome, is allowed into the
//! video) and (b) logged; the log is written next to the movie as
//! `<stem>.events.json` once the file is finalised.
//!
//! With the keystroke HUD on (Settings → Recording, off by default), a
//! listen-only event tap feeds shortcuts and special keys to a pill that is
//! also allowed into the video. Plain typing is never shown and nothing is
//! stored; the tap exists only while recording. Key characters follow the
//! keyboard layout (read at the start of each recording and again whenever
//! the input source changes), with the US table as fallback.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use lumora_capture::content::ShareableContent;
use lumora_capture::keys::{self, KeyEvent, KeyTap};
use lumora_capture::mic::{self, MicPermission};
use lumora_capture::recorder::{RecordOptions, Recorder, RecorderEvent};
use lumora_capture::CaptureError;
use lumora_core::click_log::{self, AppContext, DisplayGeometry, EventLog, Modifiers, RawClick, RecordingGeometry};
use lumora_core::filename::{render, unique_path, CaptureKind};
use lumora_core::geometry::Rect;
use lumora_core::geometry::{flip_point_y, Point};
use lumora_core::keystroke::{HudPosition, KeyMods, KeystrokeFeed};
use lumora_core::recording::{Event, Phase};
use lumora_core::selection::Target;
use lumora_ui::click_overlay::{ClickInfo, ClickMonitor, ClickOverlay};
use lumora_ui::countdown::Countdown;
use lumora_ui::key_layout::LayoutTranslator;
use lumora_ui::keystroke_hud::KeystrokeHud;
use lumora_ui::recording_hud::{RecordingBorder, StopControl};
use lumora_ui::util;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};

use lumora_core::i18n::{tr, Msg};

use crate::app::{on_main, App, Mode, Notice, RecordTarget};
use crate::system;

/// Click visualisation and logging for one recording.
pub struct ClickSession {
    _monitor: ClickMonitor,
    overlay: Option<Rc<ClickOverlay>>,
    log: ClickLogData,
    /// Keystroke HUD + its event tap (when enabled and permitted).
    keys: Option<KeystrokeSession>,
}

/// Keystroke HUD for one recording; dropping it removes the tap and panel.
struct KeystrokeSession {
    _tap: KeyTap,
    hud: Rc<KeystrokeHud>,
}

impl KeystrokeSession {
    /// `None` when the event tap can't be created (no Input Monitoring).
    fn start(mtm: objc2::MainThreadMarker, area: Rect, position: HudPosition) -> Option<KeystrokeSession> {
        let hud = KeystrokeHud::show(mtm, area, position);
        let feed = RefCell::new(KeystrokeFeed::new());
        // Layouts are read now (each session) and after input-source changes.
        let layout = LayoutTranslator::new();
        let (current, ascii) = layout.source_ids();
        log::info!("keystroke HUD layouts: {current:?} (⌘/⌃: {ascii:?})");
        let layout = RefCell::new(layout);
        let h = hud.clone();
        let tap = KeyTap::start(move |e: KeyEvent| {
            let Ok(mut feed) = feed.try_borrow_mut() else { return };
            let label = match e {
                KeyEvent::KeyDown { keycode, flags, .. } => {
                    let Ok(mut layout) = layout.try_borrow_mut() else { return };
                    feed.key_down_with(keycode, KeyMods::from_cg_flags(flags), util::now(), &mut *layout)
                }
                KeyEvent::FlagsChanged { flags } => feed.flags_changed(KeyMods::from_cg_flags(flags), util::now()),
            };
            if let Some(label) = label {
                h.display(&label);
            }
        })?;
        Some(KeystrokeSession { _tap: tap, hud })
    }
}

impl std::fmt::Debug for ClickSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClickSession")
    }
}

/// What survives the end of recording to write the sidecar.
#[derive(Clone)]
pub struct ClickLogData {
    raw: Rc<RefCell<Vec<RawClick>>>,
    /// Host-clock time of the first video frame.
    start_host: Rc<Cell<Option<f64>>>,
    /// Host-clock time stop was pressed (`CACurrentMediaTime` and
    /// `NSEvent.timestamp` share the host clock with SC sample times).
    stop_host: Rc<Cell<Option<f64>>>,
    geometry: Rc<RefCell<Option<RecordingGeometry>>>,
}

impl ClickSession {
    fn start(mtm: objc2::MainThreadMarker, show_ripples: bool) -> ClickSession {
        let overlay = show_ripples.then(|| Rc::new(ClickOverlay::show(mtm)));
        let log = ClickLogData {
            raw: Rc::default(),
            start_host: Rc::new(Cell::new(None)),
            stop_host: Rc::new(Cell::new(None)),
            geometry: Rc::new(RefCell::new(None)),
        };
        let (ov, raw) = (overlay.clone(), log.raw.clone());
        let monitor = ClickMonitor::start(mtm, move |c: ClickInfo| {
            if let Some(o) = &ov {
                o.ripple(c.point, c.button);
            }
            let point = flip_point_y(c.point, util::primary_height(mtm));
            raw.borrow_mut().push(RawClick {
                host_time: c.timestamp,
                point,
                button: c.button,
                click_count: c.click_count,
                modifiers: Modifiers::from_ns_bits(c.modifier_bits),
                app: app_context_at(point),
            });
        });
        ClickSession { _monitor: monitor, overlay, log, keys: None }
    }

    fn allow_windows(&self) -> Vec<u32> {
        let mut v = self.overlay.as_ref().map(|o| o.window_numbers()).unwrap_or_default();
        if let Some(k) = &self.keys {
            v.extend(k.hud.window_numbers());
        }
        v
    }
}

/// App + window under the click (window server info, no Accessibility),
/// falling back to the frontmost app.
fn app_context_at(point: Point) -> AppContext {
    let hit = lumora_capture::content::window_at(point);
    let app = match &hit {
        Some(h) => NSRunningApplication::runningApplicationWithProcessIdentifier(h.pid),
        None => NSWorkspace::sharedWorkspace().frontmostApplication(),
    };
    AppContext {
        bundle_id: app.as_ref().and_then(|a| a.bundleIdentifier()).map(|s| s.to_string()),
        name: app
            .as_ref()
            .and_then(|a| a.localizedName())
            .map(|s| s.to_string())
            .or_else(|| hit.as_ref().and_then(|h| h.owner_name.clone())),
        window_title: hit.and_then(|h| h.title),
    }
}

/// Write `<stem>.events.json` next to the movie (best effort).
fn write_event_log(movie: &Path, data: &ClickLogData, fallback_duration: f64) {
    let (Some(start), Some(geometry)) = (data.start_host.get(), data.geometry.borrow().clone()) else { return };
    let duration = data.stop_host.get().map_or(fallback_duration, |stop| (stop - start).max(0.0));
    let log = EventLog::build(movie, &geometry, start, duration, &data.raw.borrow());
    let path = click_log::sidecar_path(movie);
    match std::fs::write(&path, log.to_json()) {
        Ok(()) => log::info!("wrote {} ({} clicks)", path.display(), log.clicks.len()),
        Err(e) => log::warn!("could not write {}: {e}", path.display()),
    }
}

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
            _ => {
                log::info!("record toggle refused: busy ({})", self.mode.borrow().name());
                self.notify(
                    crate::app::Notice::Warning(lumora_core::i18n::Msg::ToastBusy),
                    Some(lumora_core::i18n::tr(lumora_core::i18n::Msg::ToastBusyDetail).into()),
                );
            }
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
        let (show, keystrokes, position) = {
            let r = &self.settings.borrow().recording;
            (r.show_clicks, r.keystroke_hud, r.keystroke_hud_position)
        };
        let mut clicks = Box::new(ClickSession::start(self.mtm, show));
        if keystrokes {
            clicks.keys = self.start_keystrokes(target.appkit_frame, position);
        }
        self.set_mode(Mode::Recording { recorder: None, target, _border: border, stop, timer, clicks });
        // Fetch content *after* our chrome is on screen so it gets excluded.
        ShareableContent::fetch(move |res| on_main(move |app| app.on_content_for_recording(res)));
    }

    /// Keystroke HUD: needs Input Monitoring. Without it, ask (the system
    /// prompt appears once) and record without keystrokes — never block.
    fn start_keystrokes(self: &Rc<Self>, area: Rect, position: HudPosition) -> Option<KeystrokeSession> {
        let session = if keys::has_input_monitoring() {
            KeystrokeSession::start(self.mtm, area, position)
        } else {
            keys::request_input_monitoring();
            None
        };
        if session.is_none() {
            log::warn!("keystroke HUD unavailable (Input Monitoring not granted)");
            self.notify(Notice::Warning(Msg::ToastKeystrokesUnavailable), Some(tr(Msg::KeystrokesAccessDetail).into()));
        }
        session
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
        let (target, allow_windows) = match &*self.mode.borrow() {
            Mode::Recording { target, recorder: None, clicks, .. } => (*target, clicks.allow_windows()),
            _ => return, // stopped/cancelled meanwhile
        };
        let mut s = self.settings.borrow().clone();
        if s.recording.microphone {
            match mic::permission() {
                MicPermission::NotDetermined => {
                    // Ask first, then come back with the same content.
                    mic::request_permission(move |_| on_main(move |app| app.on_content_for_recording(Ok(content))));
                    return;
                }
                MicPermission::Denied => {
                    s.recording.microphone = false;
                    self.notify(
                        Notice::Warning(Msg::ToastMicUnavailable),
                        Some(crate::capture_flow::error_text(&CaptureError::MicrophoneDenied)),
                    );
                }
                MicPermission::Granted => {}
            }
        }
        let displays: Vec<DisplayGeometry> = content
            .displays()
            .into_iter()
            .map(|d| DisplayGeometry { id: d.id, frame: d.frame, scale: d.scale })
            .collect();
        let stem = render(&s.general.filename_template, &system::now_parts(), CaptureKind::Recording);
        let dir = if s.general.save_to_disk { s.save_folder(&self.home) } else { system::temp_dir() };
        let _ = std::fs::create_dir_all(&dir);
        let output = unique_path(&dir, &stem, CaptureKind::Recording.extension(), |p| p.exists());
        let opts = RecordOptions {
            display_id: target.display_id,
            area: target.area,
            settings: s.recording.clone(),
            output,
            allow_windows,
        };
        match Recorder::start(&content, opts, |event| on_main(move |app| app.on_recorder_event(event))) {
            Ok(r) => {
                if let Mode::Recording { recorder, clicks, .. } = &mut *self.mode.borrow_mut() {
                    let (video_width, video_height) = r.video_size();
                    *clicks.log.geometry.borrow_mut() = Some(RecordingGeometry {
                        displays,
                        display_id: target.display_id,
                        capture_frame: r.capture_frame(),
                        video_width,
                        video_height,
                    });
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
            RecorderEvent::Started { host_time } => {
                let _ = self.session.borrow_mut().apply(Event::Started { now: util::now() });
                if let Mode::Recording { clicks, .. } = &*self.mode.borrow() {
                    clicks.log.start_host.set(Some(host_time));
                }
            }
            RecorderEvent::MicUnavailable(e) => {
                self.notify(Notice::Warning(Msg::ToastMicUnavailable), Some(crate::capture_flow::error_text(&e)));
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
        let Mode::Recording { recorder, target, timer, clicks, .. } = self.mode.replace(Mode::Finishing) else {
            return;
        };
        // Monitor and overlay go away now; the log lives on until the file exists.
        let log = clicks.log.clone();
        log.stop_host.set(Some(util::now()));
        drop(clicks);
        timer.invalidate();
        self.status.set_recording(None);
        let now = util::now();
        let duration = {
            let mut session = self.session.borrow_mut();
            let _ = session.apply(Event::Stop { now });
            session.elapsed(now)
        };
        match recorder {
            Some(r) => {
                // `ClickLogData` is main-thread only: park it on the app.
                *self.pending_click_log.borrow_mut() = Some(log);
                r.stop(move |res| on_main(move |app| app.on_recording_finished(res, target, duration)))
            }
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
        let click_log = self.pending_click_log.borrow_mut().take();
        let path = match res {
            Ok(p) => p,
            Err(e) => return self.fail(e),
        };
        log::info!("recorded {} ({duration:.1}s)", path.display());
        self.history_note(path.clone());
        if let Some(data) = click_log {
            write_event_log(&path, &data, duration);
        }
        let s = self.settings.borrow().clone();
        if s.general.copy_to_clipboard {
            lumora_capture::clipboard::copy_file(&path);
        }
        let frame: Rect = target.area.unwrap_or_else(|| util::flip(target.appkit_frame, self.mtm));
        let temp = !s.general.save_to_disk;
        // Poster frame off the main thread, then show the card.
        self.show_recording_card(path, temp, Some(frame), duration);
    }
}
