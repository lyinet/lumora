//! Trim and GIF export for recordings (Quick Access "Trim / GIF…" and the
//! history window).
//!
//! The trim window picks a range; exports run on a global queue with a
//! cancel flag and report progress back to the window. A trim of a saved
//! recording writes `<name> (trimmed).<ext>` next to it; a temporary
//! recording (Save to disk off) is replaced in place. The click log and
//! transcript are shifted to the new timeline; tutorial steps (which point
//! at frames of the old movie) are not carried over. GIFs go next to the
//! movie, or to the captures folder for temporary recordings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_capture::recorder;
use lumora_capture::CaptureError;
use lumora_core::clip::{self, GifPlan, GifWidth, TrimRange};
use lumora_core::geometry::{Rect, Size};
use lumora_core::i18n::{tr, trf, Msg};
use lumora_ui::events::TrimAction;
use lumora_ui::quick_access::CardContent;
use lumora_ui::toast::ToastKind;
use lumora_ui::trim_window::TrimWindow;

use crate::app::{on_main, App, CaptureRecord};

/// An open trim window and what it works on.
pub struct OpenTrim {
    pub window: TrimWindow,
    pub movie: PathBuf,
    /// A temporary recording: trimming replaces it in place.
    pub temp: bool,
    /// The recorded area (card size); `None` when opened from history.
    pub frame: Option<Rect>,
    pub info: lumora_capture::clip::MovieInfo,
    /// The running export's cancel flag.
    pub job: Option<Arc<AtomicBool>>,
    /// A preview frame is being generated.
    pub preview_busy: bool,
    /// The newest preview time asked for while one was being generated.
    pub preview_next: Option<f64>,
}

/// Open trim windows by id.
pub type Trims = HashMap<u64, OpenTrim>;

/// Run `f` on a user-initiated global queue.
fn global(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(f);
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    /// Open (or focus) the trim window for `movie`.
    pub fn open_trim(self: &Rc<Self>, movie: PathBuf, temp: bool, frame: Option<Rect>) {
        if let Some(t) = self.trims.borrow().values().find(|t| t.movie == movie) {
            t.window.focus();
            return;
        }
        global(move || {
            let info = lumora_capture::clip::movie_info(&movie);
            let poster = recorder::thumbnail(&movie, 1000.0);
            on_main(move |app| match info {
                Ok(info) => {
                    let id = app.next_id();
                    let window = TrimWindow::show(
                        app.mtm,
                        id,
                        &file_name(&movie),
                        info.duration,
                        (info.width, info.height),
                        poster,
                        app.sink.clone(),
                    );
                    app.trims.borrow_mut().insert(
                        id,
                        OpenTrim {
                            window,
                            movie,
                            temp,
                            frame,
                            info,
                            job: None,
                            preview_busy: false,
                            preview_next: None,
                        },
                    );
                }
                Err(e) => app.notify_text(ToastKind::Error, tr(Msg::ToastTrimFailed), Some(&e.to_string())),
            });
        });
    }

    pub fn on_trim(self: &Rc<Self>, id: u64, action: TrimAction) {
        match action {
            TrimAction::Preview(t) => self.trim_preview(id, t),
            TrimAction::Save(range) => self.trim_save(id, range),
            TrimAction::ExportGif { range, width, fps } => self.trim_gif(id, range, width, fps),
            TrimAction::Cancel => {
                if let Some(job) = self.trims.borrow().get(&id).and_then(|t| t.job.as_ref()) {
                    job.store(true, Ordering::Relaxed);
                }
            }
            TrimAction::Closed => {
                if let Some(t) = self.trims.borrow_mut().remove(&id) {
                    if let Some(job) = t.job {
                        job.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    /// One preview frame at a time; the newest request wins.
    fn trim_preview(self: &Rc<Self>, id: u64, time: f64) {
        let movie = {
            let mut trims = self.trims.borrow_mut();
            let Some(t) = trims.get_mut(&id) else { return };
            if t.preview_busy {
                t.preview_next = Some(time);
                return;
            }
            t.preview_busy = true;
            t.movie.clone()
        };
        global(move || {
            let image = lumora_capture::clip::frame_at(&movie, time, 1000.0);
            on_main(move |app| {
                let next = {
                    let mut trims = app.trims.borrow_mut();
                    let Some(t) = trims.get_mut(&id) else { return };
                    if let Some(img) = &image {
                        t.window.set_preview(img);
                    }
                    t.preview_busy = false;
                    t.preview_next.take()
                };
                if let Some(next) = next {
                    app.trim_preview(id, next);
                }
            });
        });
    }

    /// Start a job for window `id`: lock the window, return the cancel flag
    /// (`None` when one is already running or the window is gone).
    fn trim_begin(&self, id: u64, status: Msg) -> Option<Arc<AtomicBool>> {
        let mut trims = self.trims.borrow_mut();
        let t = trims.get_mut(&id)?;
        if t.job.is_some() {
            drop(trims);
            self.notify(crate::app::Notice::Warning(Msg::ToastClipBusy), None);
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        t.job = Some(flag.clone());
        t.window.set_busy(Some(tr(status)));
        Some(flag)
    }

    /// Unlock window `id` after a job.
    fn trim_end(&self, id: u64) {
        if let Some(t) = self.trims.borrow_mut().get_mut(&id) {
            t.job = None;
            t.window.set_busy(None);
        }
    }

    fn progress_reporter(id: u64) -> impl Fn(f32) {
        // Throttle main-queue hops to ~1 % steps.
        let last = std::sync::atomic::AtomicU32::new(u32::MAX);
        move |f: f32| {
            let step = (f.clamp(0.0, 1.0) * 100.0) as u32;
            if last.swap(step, Ordering::Relaxed) != step {
                on_main(move |app| {
                    if let Some(t) = app.trims.borrow().get(&id) {
                        t.window.set_progress(f);
                    }
                });
            }
        }
    }

    fn trim_save(self: &Rc<Self>, id: u64, range: TrimRange) {
        let Some((movie, temp, frame, duration)) =
            self.trims.borrow().get(&id).map(|t| (t.movie.clone(), t.temp, t.frame, t.info.duration))
        else {
            return;
        };
        let range = range.clamped(duration);
        if range.is_full(duration) {
            self.notify(crate::app::Notice::Warning(Msg::ToastTrimNothing), None);
            return;
        }
        let Some(cancel) = self.trim_begin(id, Msg::ClipTrimming) else { return };
        let dest = clip::trimmed_path(&movie, |p| p.exists());
        global(move || {
            let progress = App::progress_reporter(id);
            let result = lumora_capture::clip::export_trim(&movie, &dest, range, &cancel, &progress)
                .and_then(|()| finish_trim(&movie, &dest, temp, range));
            on_main(move |app| {
                app.trim_end(id);
                match result {
                    Ok((out, steps_dropped)) => {
                        let name = file_name(&out);
                        let detail =
                            if steps_dropped { trf(Msg::ToastTrimStepsDropped, &[&name]) } else { name.clone() };
                        app.notify_text(ToastKind::Success, tr(Msg::ToastTrimDone), Some(&detail));
                        if !temp {
                            app.history_note(out.clone());
                        }
                        if let Some(t) = app.trims.borrow_mut().remove(&id) {
                            t.window.close();
                        }
                        app.show_recording_card(out, temp, frame, range.len());
                    }
                    Err(CaptureError::Cancelled) => {
                        app.notify(crate::app::Notice::Warning(Msg::ToastClipCancelled), None)
                    }
                    Err(e) => app.notify_text(ToastKind::Error, tr(Msg::ToastTrimFailed), Some(&e.to_string())),
                }
            });
        });
    }

    fn trim_gif(self: &Rc<Self>, id: u64, range: TrimRange, width: GifWidth, fps: u32) {
        let Some((movie, temp, info)) = self.trims.borrow().get(&id).map(|t| (t.movie.clone(), t.temp, t.info)) else {
            return;
        };
        let range = range.clamped(info.duration);
        let plan = GifPlan::new(range, fps, clip::gif_size(info.width, info.height, width));
        // Temporary recordings live in the scratch folder: put the GIF in
        // the captures folder instead.
        let base = if temp {
            self.settings.borrow().save_folder(&self.home).join(movie.file_name().unwrap_or_default())
        } else {
            movie.clone()
        };
        if let Some(dir) = base.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let dest = clip::gif_path(&base, !range.is_full(info.duration), |p| p.exists());
        let Some(cancel) = self.trim_begin(id, Msg::ClipMakingGif) else { return };
        global(move || {
            let progress = App::progress_reporter(id);
            let result = lumora_capture::clip::export_gif(&movie, &dest, &plan, &cancel, &progress);
            on_main(move |app| {
                app.trim_end(id);
                match result {
                    Ok(()) => {
                        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                        let detail = format!("{} · {}", file_name(&dest), lumora_core::format::file_size(size));
                        app.notify_text(ToastKind::Success, tr(Msg::ToastGifSaved), Some(&detail));
                        crate::system::reveal_in_finder(&dest);
                    }
                    Err(CaptureError::Cancelled) => {
                        app.notify(crate::app::Notice::Warning(Msg::ToastClipCancelled), None)
                    }
                    Err(e) => app.notify_text(ToastKind::Error, tr(Msg::ToastGifFailed), Some(&e.to_string())),
                }
            });
        });
    }

    /// A Quick Access card (and capture record) for a finished recording.
    /// The poster frame and tutorial check run off the main thread.
    pub(crate) fn show_recording_card(self: &Rc<Self>, path: PathBuf, temp: bool, frame: Option<Rect>, duration: f64) {
        let id = self.next_id();
        let show_card = self.settings.borrow().general.show_quick_access;
        global(move || {
            let thumb = recorder::thumbnail(&path, 640.0);
            let tutorial = App::can_make_tutorial(&path);
            // Opened from history: size the card from the video (2× pixels).
            let frame = frame.unwrap_or_else(|| {
                let (w, h) = recorder::video_dimensions(&path).unwrap_or((1280, 720));
                Rect::new(0.0, 0.0, w as f64 / 2.0, h as f64 / 2.0)
            });
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
                        editable: None,
                    },
                );
                if show_card {
                    app.quick.push(CardContent {
                        id,
                        image: thumb,
                        point_size: Size::new(frame.width(), frame.height()),
                        file: path,
                        video_duration: Some(duration),
                        tutorial,
                    });
                }
            });
        });
    }
}

/// After the export: rewrite the sidecars for the new timeline and, for a
/// temporary recording, replace the original. Returns the final movie path
/// and whether tutorial steps were dropped. Runs off the main thread.
fn finish_trim(movie: &Path, trimmed: &Path, temp: bool, range: TrimRange) -> Result<(PathBuf, bool), CaptureError> {
    let write_err =
        |p: &Path, e: std::io::Error| CaptureError::Write { path: p.display().to_string(), reason: e.to_string() };
    let out = if temp { movie.to_path_buf() } else { trimmed.to_path_buf() };
    let movie_name = file_name(&out);

    // Read the old sidecars before a temp replace overwrites anything.
    let events = std::fs::read_to_string(lumora_core::click_log::sidecar_path(movie))
        .ok()
        .and_then(|j| lumora_core::click_log::EventLog::from_json(&j).ok())
        .map(|log| clip::trim_event_log(&log, range, &movie_name));
    let transcript = std::fs::read_to_string(lumora_core::transcript::sidecar_path(movie))
        .ok()
        .and_then(|j| lumora_core::transcript::Transcript::from_json(&j).ok())
        .map(|t| clip::trim_transcript(&t, range));
    let steps = lumora_core::tutorial::sidecar_path(movie);
    let had_steps = steps.exists();

    if temp {
        std::fs::rename(trimmed, movie).map_err(|e| write_err(movie, e))?;
        // The old steps point at frames that may be gone: drop them.
        let _ = std::fs::remove_file(&steps);
        let _ = std::fs::remove_dir_all(lumora_core::tutorial::image_dir(movie));
    }
    let events_path = lumora_core::click_log::sidecar_path(&out);
    if let Some(log) = events {
        if let Err(e) = std::fs::write(&events_path, log.to_json()) {
            log::warn!("could not write {}: {e}", events_path.display());
        }
    }
    let transcript_path = lumora_core::transcript::sidecar_path(&out);
    match transcript {
        Some(Some(t)) => match t.to_json() {
            Ok(json) => {
                if let Err(e) = std::fs::write(&transcript_path, json) {
                    log::warn!("could not write {}: {e}", transcript_path.display());
                }
            }
            Err(e) => log::warn!("transcript JSON: {e}"),
        },
        // No speech left in the clip: no transcript (remove a stale one).
        Some(None) if temp => {
            let _ = std::fs::remove_file(&transcript_path);
        }
        _ => {}
    }
    Ok((out, had_steps))
}
