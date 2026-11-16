//! Recording → subtitles, summary and chapters.
//!
//! Entry points: the "Subtitles" button on a recording's Quick Access card
//! and the menu-bar item "Transcribe Recording…" (which turns into "Cancel
//! Transcription" while one runs). A recording that already has a
//! `<stem>.transcript.json` reopens it (with its edits).
//!
//! Pipeline, on a background queue: Speech permission (on-device engine,
//! asked on the main thread first) → export the chosen audio to a temp
//! `.m4a` → recognise (Apple Speech on this Mac, or the AI provider's
//! `/audio/transcriptions`) → normalise → save the sidecar → summary and
//! chapters (AI when enabled, else local) → save → [`TranscriptWindow`].
//! The temp audio is always deleted. Every stage checks the cancel flag.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_ai::{KeyStore, Keychain, TranscribeError, TranscribeRequest, MAX_UPLOAD_BYTES};
use lumora_capture::transcribe::{self as speech, AudioTrack, SpeechAuthorization};
use lumora_capture::CaptureError;
use lumora_core::ai::AiSettings;
use lumora_core::click_log::{self, EventLog};
use lumora_core::i18n::{language, tr, trf, Language, Msg};
use lumora_core::settings::{Transcribe, TranscribeAudio, TranscribeEngine};
use lumora_core::transcript::{self, chapters, Segment, Transcript};
use lumora_ui::events::TranscriptAction;
use lumora_ui::toast::ToastKind;
use lumora_ui::transcript_window::TranscriptWindow;
use lumora_ui::util;

use crate::app::{on_main, App};
use crate::capture_flow::error_text;

/// An open transcript window and the movie it belongs to.
pub struct OpenTranscript {
    pub window: TranscriptWindow,
    /// Where the movie is now (updated when Save moves it).
    pub movie: PathBuf,
}

/// How the background run ended.
enum Outcome {
    Ready {
        transcript: Box<Transcript>,
        ai_failed: bool,
    },
    NoSpeech,
    Cancelled,
    /// Localised title + detail.
    Failed {
        title: String,
        detail: Option<String>,
    },
}

/// Everything the worker needs, captured on the main thread.
struct Job {
    movie: PathBuf,
    cfg: Transcribe,
    ai: AiSettings,
    ui: Language,
    cancel: Arc<AtomicBool>,
}

fn stem(movie: &Path) -> String {
    movie.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Recording".into())
}

fn base_dir(movie: &Path) -> PathBuf {
    movie.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn save_transcript(movie: &Path, t: &Transcript) {
    let path = transcript::sidecar_path(movie);
    match t.to_json() {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::warn!("could not write {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("could not serialise the transcript: {e}"),
    }
}

fn load_transcript(movie: &Path) -> Option<Transcript> {
    let path = transcript::sidecar_path(movie);
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| Transcript::from_json(&s).map_err(|e| e.to_string()))
    {
        Ok(t) => Some(t),
        Err(e) => {
            log::warn!("ignoring unreadable {}: {e}", path.display());
            None
        }
    }
}

/// Click times (seconds into the movie) from the click log, if any.
fn click_times(movie: &Path) -> Vec<f64> {
    std::fs::read_to_string(click_log::sidecar_path(movie))
        .ok()
        .and_then(|s| EventLog::from_json(&s).ok())
        .map(|log| log.clicks.iter().filter(|c| c.inside_recording).map(|c| c.t).collect())
        .unwrap_or_default()
}

fn track(source: TranscribeAudio) -> AudioTrack {
    match source {
        TranscribeAudio::Mic => AudioTrack::Mic,
        TranscribeAudio::System => AudioTrack::System,
        TranscribeAudio::Mixed => AudioTrack::Mixed,
    }
}

/// Drop blank segments (emptied in the window) before exporting.
fn spoken(segments: &[Segment]) -> Vec<Segment> {
    segments.iter().filter(|s| !s.text.trim().is_empty()).cloned().collect()
}

fn capture_failure(e: &CaptureError) -> Outcome {
    match e {
        CaptureError::Cancelled => Outcome::Cancelled,
        _ => Outcome::Failed { title: tr(Msg::ToastTranscribeFailed).to_string(), detail: Some(error_text(e)) },
    }
}

/// Recognise `audio` with the configured engine. Blocking.
fn recognise(job: &Job, audio: &Path) -> Result<Vec<Segment>, Outcome> {
    match job.cfg.engine {
        TranscribeEngine::OnDevice => {
            let locale = job.cfg.language.locale(job.ui);
            speech::transcribe_on_device(audio, locale, job.cfg.strict_on_device, job.cancel.clone())
                .map_err(|e| capture_failure(&e))
        }
        TranscribeEngine::Cloud => {
            let mut ai = job.ai.clone();
            ai.validate();
            if !ai.enabled {
                return Err(Outcome::Failed { title: tr(Msg::ToastTranscribeCloudOff).to_string(), detail: None });
            }
            let bytes = std::fs::read(audio).map_err(|e| Outcome::Failed {
                title: tr(Msg::ToastTranscribeFailed).to_string(),
                detail: Some(e.to_string()),
            })?;
            if bytes.len() > MAX_UPLOAD_BYTES {
                return Err(too_large(bytes.len()));
            }
            let key = Keychain.get(ai.provider).map_err(|e| Outcome::Failed {
                title: tr(Msg::ToastTranscribeFailed).to_string(),
                detail: Some(e.user_message()),
            })?;
            if job.cancel.load(Ordering::Relaxed) {
                return Err(Outcome::Cancelled);
            }
            on_main(|app| app.notify_text(ToastKind::Success, tr(Msg::ToastTranscribeWorking), None));
            let req = TranscribeRequest {
                model: job.cfg.cloud_model.clone(),
                language: Some(job.cfg.language.iso639(job.ui).to_string()),
                prompt: None,
            };
            let result = lumora_ai::transcribe(&ai, key.as_deref(), &bytes, "audio.m4a", &req);
            if job.cancel.load(Ordering::Relaxed) {
                return Err(Outcome::Cancelled);
            }
            result.map_err(|e| match e {
                TranscribeError::FileTooLarge { bytes } => too_large(bytes),
                TranscribeError::Ai(e) => Outcome::Failed {
                    title: tr(Msg::ToastTranscribeFailed).to_string(),
                    detail: Some(e.user_message()),
                },
            })
        }
    }
}

fn too_large(bytes: usize) -> Outcome {
    let mb = format!("{:.1}", bytes as f64 / (1024.0 * 1024.0));
    Outcome::Failed {
        title: trf(Msg::ToastTranscribeTooLarge, &[&mb]),
        detail: Some(tr(Msg::ToastTranscribeTooLargeDetail).to_string()),
    }
}

/// Summary + chapters: AI when enabled and ready, else local. Returns
/// whether AI was wanted but failed. Blocking.
fn summarise(job: &Job, t: &mut Transcript) -> bool {
    let clicks = click_times(&job.movie);
    let local = chapters::local_summary(&t.segments, &clicks);
    let mut cfg = job.ai.clone();
    cfg.validate();
    if !cfg.enabled {
        t.summary = Some(local);
        return false;
    }
    let key = match Keychain.get(cfg.provider) {
        Ok(key) if cfg.check_ready(key.as_deref()).is_ok() => key,
        Ok(_) => {
            t.summary = Some(local);
            return true;
        }
        Err(e) => {
            log::warn!("reading the {} API key failed: {e:?}", cfg.provider.id());
            t.summary = Some(local);
            return true;
        }
    };
    on_main(|app| app.notify_text(ToastKind::Success, tr(Msg::ToastTranscribeSummarizing), None));
    let raw = chapters::build_request(&cfg, &t.segments, &clicks, job.ui)
        .and_then(|req| lumora_ai::complete(&cfg, key.as_deref(), &req, &job.cancel, |_| {}));
    match raw {
        Ok(raw) => {
            t.summary = Some(chapters::summary_from_ai(&raw, &t.segments, &clicks));
            false
        }
        Err(e) => {
            log::warn!("transcript summary failed: {e:?}");
            t.summary = Some(local);
            !job.cancel.load(Ordering::Relaxed)
        }
    }
}

/// The whole background run.
fn run(job: &Job) -> Outcome {
    let audio = match speech::export_audio(&job.movie, track(job.cfg.audio_source)) {
        Ok(a) => a,
        Err(e) => return capture_failure(&e),
    };
    if job.cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&audio);
        return Outcome::Cancelled;
    }
    if job.cfg.engine == TranscribeEngine::OnDevice {
        on_main(|app| app.notify_text(ToastKind::Success, tr(Msg::ToastTranscribeWorking), None));
    }
    let result = recognise(job, &audio);
    let _ = std::fs::remove_file(&audio);
    let raw = match result {
        Ok(s) => s,
        Err(o) => return o,
    };
    if job.cancel.load(Ordering::Relaxed) {
        return Outcome::Cancelled;
    }
    let segments = transcript::normalize(&raw);
    if segments.iter().all(|s| s.text.trim().is_empty()) {
        return Outcome::NoSpeech;
    }
    let engine = match job.cfg.engine {
        TranscribeEngine::OnDevice => "apple-speech".to_string(),
        TranscribeEngine::Cloud => format!("cloud:{}", job.cfg.cloud_model),
    };
    let mut t = Transcript::new(job.cfg.language.locale(job.ui), engine, segments);
    save_transcript(&job.movie, &t);
    let ai_failed = summarise(job, &mut t);
    if job.cancel.load(Ordering::Relaxed) {
        // The transcript is saved; the summary is local. Still show it.
        log::info!("summary cancelled; keeping the transcript");
    }
    save_transcript(&job.movie, &t);
    Outcome::Ready { transcript: Box::new(t), ai_failed }
}

impl App {
    /// Menu: cancel running transcriptions, or pick a recording and
    /// transcribe (or reopen) it.
    pub fn choose_recording_for_transcript(self: &Rc<Self>) {
        if self.cancel_transcriptions() {
            return;
        }
        let Some(movie) = util::choose_file_with_extensions(self.mtm, &["mp4", "mov"]) else { return };
        self.open_or_transcribe(movie);
    }

    /// Stop every running transcription; `false` when none was running.
    fn cancel_transcriptions(self: &Rc<Self>) -> bool {
        let jobs = self.transcribe_jobs.borrow();
        if jobs.is_empty() {
            return false;
        }
        for cancel in jobs.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        true
    }

    /// Focus an open window, reopen a saved transcript, cancel a running
    /// job for this movie, or start one.
    pub fn open_or_transcribe(self: &Rc<Self>, movie: PathBuf) {
        if let Some(open) = self.transcripts.borrow().values().find(|o| o.movie == movie) {
            open.window.focus();
            return;
        }
        if let Some(cancel) = self.transcribe_jobs.borrow().get(&movie) {
            cancel.store(true, Ordering::Relaxed);
            return;
        }
        if let Some(t) = load_transcript(&movie) {
            return self.open_transcript_window(movie, t);
        }
        self.transcribe(movie);
    }

    /// Start transcribing `movie` (asks for Speech permission first when
    /// the on-device engine needs it).
    pub fn transcribe(self: &Rc<Self>, movie: PathBuf) {
        let engine = self.settings.borrow().transcribe.engine;
        if engine == TranscribeEngine::OnDevice {
            match speech::speech_authorization() {
                SpeechAuthorization::Authorized => {}
                SpeechAuthorization::NotDetermined => {
                    speech::request_speech_authorization(move |status| {
                        on_main(move |app| {
                            if status == SpeechAuthorization::Authorized {
                                app.start_transcription(movie);
                            } else {
                                app.transcribe_failed_with(&CaptureError::SpeechDenied);
                            }
                        });
                    });
                    return;
                }
                SpeechAuthorization::Denied => return self.transcribe_failed_with(&CaptureError::SpeechDenied),
                SpeechAuthorization::Restricted => {
                    return self.transcribe_failed_with(&CaptureError::SpeechUnavailable(
                        "speech recognition is restricted on this Mac".into(),
                    ))
                }
            }
        }
        self.start_transcription(movie);
    }

    fn transcribe_failed_with(self: &Rc<Self>, e: &CaptureError) {
        self.notify_text(ToastKind::Error, tr(Msg::ToastTranscribeFailed), Some(&error_text(e)));
    }

    fn start_transcription(self: &Rc<Self>, movie: PathBuf) {
        if self.transcribe_jobs.borrow().contains_key(&movie) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.transcribe_jobs.borrow_mut().insert(movie.clone(), cancel.clone());
        self.status.set_transcribing(true);
        let (cfg, ai) = {
            let s = self.settings.borrow();
            (s.transcribe.clone(), s.ai.clone())
        };
        let job = Job { movie: movie.clone(), cfg, ai, ui: language(), cancel };
        log::info!("transcribing {} ({:?})", movie.display(), job.cfg.engine);
        self.notify_text(
            ToastKind::Success,
            tr(Msg::ToastTranscribeExporting),
            Some(tr(Msg::ToastTranscribeExportingDetail)),
        );
        DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated)).exec_async(
            move || {
                let outcome = run(&job);
                let movie = job.movie;
                on_main(move |app| app.transcription_finished(movie, outcome));
            },
        );
    }

    fn transcription_finished(self: &Rc<Self>, movie: PathBuf, outcome: Outcome) {
        self.transcribe_jobs.borrow_mut().remove(&movie);
        self.status.set_transcribing(!self.transcribe_jobs.borrow().is_empty());
        match outcome {
            Outcome::Cancelled => self.notify_text(ToastKind::Warning, tr(Msg::ToastTranscribeCancelled), None),
            Outcome::NoSpeech => self.notify_text(ToastKind::Warning, tr(Msg::ToastTranscribeNoSpeech), None),
            Outcome::Failed { title, detail } => {
                log::warn!("transcription failed: {title} {}", detail.as_deref().unwrap_or(""));
                self.notify_text(ToastKind::Error, &title, detail.as_deref());
            }
            Outcome::Ready { transcript, ai_failed } => {
                if ai_failed {
                    self.notify_text(ToastKind::Warning, tr(Msg::ToastTranscribeAiFallback), None);
                } else {
                    self.notify_text(ToastKind::Success, tr(Msg::ToastTranscribeReady), None);
                }
                self.open_transcript_window(movie, *transcript);
            }
        }
    }

    fn open_transcript_window(self: &Rc<Self>, movie: PathBuf, t: Transcript) {
        if let Some(open) = self.transcripts.borrow().values().find(|o| o.movie == movie) {
            open.window.focus();
            return;
        }
        let id = self.next_id();
        let window = TranscriptWindow::show(self.mtm, id, t, &stem(&movie), self.sink.clone());
        self.transcripts.borrow_mut().insert(id, OpenTranscript { window, movie });
    }

    pub fn on_transcript(self: &Rc<Self>, id: u64, action: TranscriptAction) {
        let Some(movie) = self.transcripts.borrow().get(&id).map(|o| o.movie.clone()) else { return };
        let base = base_dir(&movie);
        match action {
            TranscriptAction::ExportSrt(t) => {
                save_transcript(&movie, &t);
                let name = format!("{}.srt", stem(&movie));
                let Some(path) = util::choose_save_path(self.mtm, &base, &name, "srt") else { return };
                let srt = transcript::to_srt(&spoken(&t.segments));
                self.report_subtitle_export(&path, std::fs::write(&path, srt.as_bytes()));
            }
            TranscriptAction::ExportVtt(t) => {
                save_transcript(&movie, &t);
                let name = format!("{}.vtt", stem(&movie));
                let Some(path) = util::choose_save_path(self.mtm, &base, &name, "vtt") else { return };
                let vtt = transcript::to_vtt(&spoken(&t.segments));
                self.report_subtitle_export(&path, std::fs::write(&path, vtt.as_bytes()));
            }
            TranscriptAction::CopyChapters(t) => {
                save_transcript(&movie, &t);
                let list = t.summary.as_ref().map(|s| chapters::chapter_list(&s.chapters)).unwrap_or_default();
                lumora_capture::clipboard::copy_text(&list);
                self.notify_text(ToastKind::Success, tr(Msg::ToastChaptersCopied), None);
            }
            TranscriptAction::CopySummary(t) => {
                save_transcript(&movie, &t);
                let md = t.summary.as_ref().map(|s| chapters::summary_markdown(s, &stem(&movie))).unwrap_or_default();
                lumora_capture::clipboard::copy_text(&md);
                self.notify_text(ToastKind::Success, tr(Msg::ToastSummaryCopied), None);
            }
            TranscriptAction::Closed(t) => {
                save_transcript(&movie, &t);
                // Drop the window after this event (its delegate is mid-close).
                let removed = self.transcripts.borrow_mut().remove(&id);
                drop(removed);
            }
        }
    }

    fn report_subtitle_export(self: &Rc<Self>, path: &Path, result: std::io::Result<()>) {
        match result {
            Ok(()) => {
                log::info!("exported subtitles to {}", path.display());
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.notify_text(ToastKind::Success, tr(Msg::ToastTranscribeExported), name.as_deref());
                crate::system::reveal_in_finder(path);
            }
            Err(e) => {
                let name = path.display().to_string();
                self.notify_text(ToastKind::Error, tr(Msg::ToastSaveFailed), Some(&format!("{name}: {e}")));
            }
        }
    }
}
