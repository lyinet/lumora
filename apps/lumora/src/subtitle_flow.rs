//! Burn subtitles into a recording ("Burn In Subtitles…" in the transcript
//! window).
//!
//! Writes `<name> (subtitled).<ext>` next to the movie (into the captures
//! folder for a temporary recording) on a global queue, with progress and
//! Cancel in the transcript window, then shows a Quick Access card. The
//! style controls are remembered in `[transcribe]`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_capture::CaptureError;
use lumora_core::i18n::{tr, Msg};
use lumora_core::transcript::burn::SubtitleStyle;
use lumora_core::transcript::Transcript;
use lumora_ui::toast::ToastKind;

use crate::app::{on_main, App, Notice};

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    /// Remember the burn-in style (saved with the settings).
    pub(crate) fn subtitle_style_changed(&self, style: SubtitleStyle) {
        {
            let mut s = self.settings.borrow_mut();
            if s.transcribe.subtitle_style() == style {
                return;
            }
            s.transcribe.set_subtitle_style(style);
        }
        if let Some(path) = &self.settings_path {
            if let Err(e) = self.settings.borrow().save(path) {
                log::error!("saving settings failed: {e}");
            }
        }
    }

    /// Cancel the burn-in running for transcript window `id`.
    pub(crate) fn cancel_burn(&self, id: u64) {
        if let Some(job) = self.transcripts.borrow().get(&id).and_then(|o| o.burn.as_ref()) {
            job.store(true, Ordering::Relaxed);
        }
    }

    /// Burn `t`'s segments into a copy of transcript window `id`'s movie.
    pub(crate) fn burn_subtitles(self: &Rc<Self>, id: u64, t: &Transcript, style: SubtitleStyle) {
        self.subtitle_style_changed(style);
        let segments: Vec<_> = t.segments.iter().filter(|s| !s.text.trim().is_empty()).cloned().collect();
        if segments.is_empty() {
            self.notify(Notice::Warning(Msg::ToastSubtitlesNone), None);
            return;
        }
        let (movie, cancel) = {
            let mut open = self.transcripts.borrow_mut();
            let Some(o) = open.get_mut(&id) else { return };
            if o.burn.is_some() {
                drop(open);
                self.notify(Notice::Warning(Msg::ToastClipBusy), None);
                return;
            }
            let flag = Arc::new(AtomicBool::new(false));
            o.burn = Some(flag.clone());
            o.window.set_busy(Some(tr(Msg::ClipBurningSubtitles)));
            (o.movie.clone(), flag)
        };
        let temp = movie.starts_with(crate::system::temp_dir());
        // Temporary recordings live in the scratch folder: write the
        // subtitled copy to the captures folder instead.
        let base = if temp {
            self.settings.borrow().save_folder(&self.home).join(movie.file_name().unwrap_or_default())
        } else {
            movie.clone()
        };
        if let Some(dir) = base.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let dest = lumora_core::clip::subtitled_path(&base, |p| p.exists());
        log::info!("burning subtitles into {}", dest.display());
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let last = AtomicU32::new(u32::MAX);
                let progress = move |f: f32| {
                    let step = (f.clamp(0.0, 1.0) * 100.0) as u32;
                    if last.swap(step, Ordering::Relaxed) != step {
                        on_main(move |app| {
                            if let Some(o) = app.transcripts.borrow().get(&id) {
                                o.window.set_progress(f);
                            }
                        });
                    }
                };
                let result =
                    lumora_capture::burn::export_subtitled(&movie, &dest, &segments, style, &cancel, &progress)
                        .map(|()| lumora_capture::clip::movie_info(&dest).map(|i| i.duration).unwrap_or(0.0));
                on_main(move |app| app.burn_finished(id, dest, result));
            },
        );
    }

    fn burn_finished(self: &Rc<Self>, id: u64, dest: PathBuf, result: Result<f64, CaptureError>) {
        if let Some(o) = self.transcripts.borrow_mut().get_mut(&id) {
            o.burn = None;
            o.window.set_busy(None);
        }
        match result {
            Ok(duration) => {
                self.notify_text(ToastKind::Success, tr(Msg::ToastSubtitlesBurned), Some(&file_name(&dest)));
                // The copy is a real file in a real folder: it goes in history.
                self.history_note(dest.clone());
                self.show_recording_card(dest, false, None, duration);
            }
            Err(CaptureError::Cancelled) => self.notify(Notice::Warning(Msg::ToastClipCancelled), None),
            Err(e) => {
                log::warn!("burn-in failed: {e}");
                self.notify_text(ToastKind::Error, tr(Msg::ToastSubtitlesBurnFailed), Some(&e.to_string()));
            }
        }
    }
}
