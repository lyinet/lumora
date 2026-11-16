//! On-device transcription: audio export (`AVAssetExportSession`) and
//! Apple Speech (`SFSpeechRecognizer`).
//!
//! The recorder writes audio tracks in this order: system audio first (when
//! enabled), then the microphone. So with two audio tracks the mic is the
//! second one; with a single audio track we cannot tell which it is and use
//! it for every [`AudioTrack`] choice.
//!
//! Speech.framework has no objc2 crate in the lockfile, so it is bound here
//! with `msg_send!` on `AnyObject` and linked explicitly.
//!
//! Everything here blocks: call it on a worker thread, never the main
//! thread. Completion handlers run on private queues, so a blocked main
//! thread is not needed (nor allowed) for progress.

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use block2::RcBlock;
use lumora_core::transcript::Segment;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send, sel};
use objc2_av_foundation::{
    AVAsset, AVAssetExportPresetAppleM4A, AVAssetExportSession, AVAssetExportSessionStatus, AVAssetTrack,
    AVFileTypeAppleM4A, AVMediaTypeAudio, AVMutableComposition,
};
use objc2_core_media::{kCMPersistentTrackID_Invalid, kCMTimeZero, CMTime, CMTimeRange};
use objc2_foundation::{NSArray, NSError, NSString};

use crate::words::{group_words, Word};
use crate::CaptureError;

#[link(name = "Speech", kind = "framework")]
extern "C" {}

/// Audio longer than this (seconds) is transcribed in chunks.
pub const CHUNK_THRESHOLD_SECS: f64 = 55.0;
/// Chunk length (seconds) for long audio.
pub const CHUNK_SECS: f64 = 50.0;
/// How often a blocked wait checks the cancel flag.
const POLL: Duration = Duration::from_millis(150);

/// Which audio of a recording to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTrack {
    /// The microphone (second audio track; the only one if there is one).
    Mic,
    /// System audio (first audio track).
    System,
    /// Every audio track mixed down.
    Mixed,
}

/// Speech Recognition (TCC) authorisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechAuthorization {
    Authorized,
    Denied,
    Restricted,
    NotDetermined,
}

impl SpeechAuthorization {
    /// From `SFSpeechRecognizerAuthorizationStatus`.
    fn from_raw(v: isize) -> Self {
        match v {
            3 => Self::Authorized,
            2 => Self::Restricted,
            1 => Self::Denied,
            _ => Self::NotDetermined,
        }
    }
}

fn speech_class(name: &CStr) -> Result<&'static AnyClass, CaptureError> {
    AnyClass::get(name).ok_or_else(|| CaptureError::SpeechUnavailable(format!("{} not found", name.to_string_lossy())))
}

fn ns_error(e: &NSError) -> String {
    e.localizedDescription().to_string()
}

/// Current speech recognition authorisation.
pub fn speech_authorization() -> SpeechAuthorization {
    let Ok(cls) = speech_class(c"SFSpeechRecognizer") else { return SpeechAuthorization::Restricted };
    // SAFETY: `+authorizationStatus` returns an NSInteger enum.
    let raw: isize = unsafe { msg_send![cls, authorizationStatus] };
    SpeechAuthorization::from_raw(raw)
}

/// Ask for speech recognition access (system prompt the first time; needs
/// `NSSpeechRecognitionUsageDescription`). `done` runs on an arbitrary queue.
pub fn request_speech_authorization(done: impl FnOnce(SpeechAuthorization) + Send + 'static) {
    let Ok(cls) = speech_class(c"SFSpeechRecognizer") else { return done(SpeechAuthorization::Restricted) };
    let slot = Mutex::new(Some(done));
    let block = RcBlock::new(move |status: isize| {
        if let Some(f) = slot.lock().ok().and_then(|mut s| s.take()) {
            f(SpeechAuthorization::from_raw(status));
        }
    });
    // SAFETY: `+requestAuthorization:` takes a `void (^)(NSInteger)` block.
    let () = unsafe { msg_send![cls, requestAuthorization: &*block] };
}

// ---------------------------------------------------------------------------
// Audio export

fn temp_m4a(label: &str) -> Result<PathBuf, CaptureError> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join("lumora-transcribe");
    std::fs::create_dir_all(&dir)
        .map_err(|e| CaptureError::Write { path: dir.display().to_string(), reason: e.to_string() })?;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{}-{nanos}-{n}-{label}.m4a", std::process::id()));
    let _ = std::fs::remove_file(&path);
    Ok(path)
}

fn audio_type() -> Result<&'static NSString, CaptureError> {
    // SAFETY: reading an AVFoundation constant.
    unsafe { AVMediaTypeAudio }.ok_or_else(|| CaptureError::Framework("AVMediaTypeAudio missing".into()))
}

/// Run `session` to completion on its own queue, blocking this thread.
fn run_export(session: &AVAssetExportSession, cancel: Option<&AtomicBool>) -> Result<(), CaptureError> {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let signal = Arc::clone(&pair);
    let block = RcBlock::new(move || {
        let (lock, cv) = &*signal;
        if let Ok(mut done) = lock.lock() {
            *done = true;
        }
        cv.notify_all();
    });
    // SAFETY: the block only touches thread-safe state.
    #[allow(deprecated)] // `exportToURL:fileType:` (async) is macOS 15+.
    unsafe {
        session.exportAsynchronouslyWithCompletionHandler(&block)
    };
    let (lock, cv) = &*pair;
    let mut done = lock.lock().map_err(|_| CaptureError::Framework("export lock poisoned".into()))?;
    while !*done {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            // SAFETY: cancelling a running export is always allowed.
            unsafe { session.cancelExport() };
        }
        done = cv.wait_timeout(done, POLL).map_err(|_| CaptureError::Framework("export lock poisoned".into()))?.0;
    }
    drop(done);
    // SAFETY: plain property reads after completion.
    let (status, error) = unsafe { (session.status(), session.error()) };
    if status == AVAssetExportSessionStatus::Completed {
        Ok(())
    } else if status == AVAssetExportSessionStatus::Cancelled || cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        Err(CaptureError::Cancelled)
    } else {
        Err(CaptureError::Framework(format!(
            "audio export failed: {}",
            error.map(|e| ns_error(&e)).unwrap_or_else(|| format!("status {}", status.0))
        )))
    }
}

/// Export `asset` (optionally only `range`) as AAC `.m4a` into a temp file.
fn export_m4a(
    asset: &AVAsset,
    range: Option<CMTimeRange>,
    label: &str,
    cancel: Option<&AtomicBool>,
) -> Result<PathBuf, CaptureError> {
    let out = temp_m4a(label)?;
    let url = crate::clipboard::file_url(&out).ok_or_else(|| CaptureError::Framework("invalid temp path".into()))?;
    // SAFETY: standard export session setup on a local asset.
    let session = unsafe {
        let session = AVAssetExportSession::exportSessionWithAsset_presetName(asset, AVAssetExportPresetAppleM4A)
            .ok_or_else(|| CaptureError::Framework("cannot export audio (M4A preset unavailable)".into()))?;
        session.setOutputURL(Some(&url));
        session.setOutputFileType(AVFileTypeAppleM4A);
        if let Some(r) = range {
            session.setTimeRange(r);
        }
        session
    };
    match run_export(&session, cancel) {
        Ok(()) => Ok(out),
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            Err(e)
        }
    }
}

/// Export the chosen audio of `movie` as AAC `.m4a` into a temp file and
/// return its path (the caller deletes it). Falls back to whichever audio
/// track exists; [`CaptureError::NoAudio`] when there is none. Blocks.
pub fn export_audio(movie: &Path, track: AudioTrack) -> Result<PathBuf, CaptureError> {
    let url = crate::clipboard::file_url(movie).ok_or_else(|| CaptureError::Framework("invalid movie path".into()))?;
    let audio = audio_type()?;
    // SAFETY: synchronous property reads on a local file asset (worker thread).
    let (asset, tracks, duration) = unsafe {
        let asset = AVAsset::assetWithURL(&url);
        #[allow(deprecated)] // the async loaders need a completion hop; we are on a worker thread.
        let tracks: Retained<NSArray<AVAssetTrack>> = asset.tracksWithMediaType(audio);
        let duration = asset.duration();
        (asset, tracks, duration)
    };
    let tracks: Vec<Retained<AVAssetTrack>> = tracks.iter().collect();
    let chosen: Vec<&AVAssetTrack> = match (track, tracks.len()) {
        (_, 0) => return Err(CaptureError::NoAudio),
        (_, 1) | (AudioTrack::System, _) => vec![&tracks[0]],
        (AudioTrack::Mic, _) => vec![&tracks[1]],
        (AudioTrack::Mixed, _) => tracks.iter().map(|t| &**t).collect(),
    };
    if chosen.len() == tracks.len() && tracks.len() == 1 {
        // Single track: export the asset directly (the video is dropped by the preset).
        return export_m4a(&asset, None, "audio", None);
    }
    // SAFETY: building a composition from tracks of a live asset.
    let composition = unsafe {
        let composition = AVMutableComposition::composition();
        let range = CMTimeRange { start: kCMTimeZero, duration };
        for src in chosen {
            let dst = composition
                .addMutableTrackWithMediaType_preferredTrackID(audio, kCMPersistentTrackID_Invalid)
                .ok_or_else(|| CaptureError::Framework("cannot add composition track".into()))?;
            dst.insertTimeRange_ofTrack_atTime_error(range, src, kCMTimeZero)
                .map_err(|e| CaptureError::Framework(ns_error(&e)))?;
        }
        composition
    };
    export_m4a(&composition, None, "audio", None)
}

// ---------------------------------------------------------------------------
// Speech

/// Outcome reported by the recognition result handler.
type Outcome = Result<Vec<Word>, (String, isize, String)>;

/// Words of `bestTranscription` of an `SFSpeechRecognitionResult`.
///
/// # Safety
/// `result` must be a valid `SFSpeechRecognitionResult`.
unsafe fn result_words(result: &AnyObject) -> Vec<Word> {
    // SAFETY: documented SFSpeech* getters.
    unsafe {
        let best: Option<Retained<AnyObject>> = msg_send![result, bestTranscription];
        let Some(best) = best else { return Vec::new() };
        let segs: Option<Retained<NSArray<AnyObject>>> = msg_send![&*best, segments];
        let Some(segs) = segs else { return Vec::new() };
        segs.iter()
            .map(|s| {
                let text: Retained<NSString> = msg_send![&*s, substring];
                let start: f64 = msg_send![&*s, timestamp];
                let duration: f64 = msg_send![&*s, duration];
                Word::new(start, duration, text.to_string())
            })
            .collect()
    }
}

/// Recognize one audio file (≤ ~1 min) and return its words.
fn recognize_file(
    recognizer: &AnyObject,
    audio: &Path,
    on_device: bool,
    cancel: &AtomicBool,
) -> Result<Vec<Word>, CaptureError> {
    let url = crate::clipboard::file_url(audio).ok_or_else(|| CaptureError::Framework("invalid audio path".into()))?;
    let req_cls = speech_class(c"SFSpeechURLRecognitionRequest")?;
    // SAFETY: SFSpeechURLRecognitionRequest initialiser and property setters.
    let request: Retained<AnyObject> = unsafe {
        let alloc: Allocated<AnyObject> = msg_send![req_cls, alloc];
        let request: Option<Retained<AnyObject>> = msg_send![alloc, initWithURL: &*url];
        let request = request.ok_or_else(|| CaptureError::Speech("cannot create recognition request".into()))?;
        let () = msg_send![&*request, setShouldReportPartialResults: false];
        let () = msg_send![&*request, setRequiresOnDeviceRecognition: on_device];
        let () = msg_send![&*request, setTaskHint: 1isize]; // SFSpeechRecognitionTaskHintDictation
        let punct: bool = msg_send![&*request, respondsToSelector: sel!(setAddsPunctuation:)];
        if punct {
            let () = msg_send![&*request, setAddsPunctuation: true];
        }
        request
    };

    let pair: Arc<(Mutex<Option<Outcome>>, Condvar)> = Arc::new((Mutex::new(None), Condvar::new()));
    let signal = Arc::clone(&pair);
    let handler = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        let outcome: Option<Outcome> = if !error.is_null() {
            // SAFETY: non-null NSError from the framework.
            let e = unsafe { &*error };
            Some(Err((e.domain().to_string(), e.code(), ns_error(e))))
        } else if !result.is_null() {
            // SAFETY: non-null SFSpeechRecognitionResult from the framework.
            let r = unsafe { &*result };
            let is_final: bool = unsafe { msg_send![r, isFinal] };
            is_final.then(|| Ok(unsafe { result_words(r) }))
        } else {
            None
        };
        if let Some(o) = outcome {
            let (lock, cv) = &*signal;
            if let Ok(mut slot) = lock.lock() {
                slot.get_or_insert(o);
            }
            cv.notify_all();
        }
    });
    // SAFETY: the block matches `void (^)(SFSpeechRecognitionResult *, NSError *)`.
    let task: Option<Retained<AnyObject>> =
        unsafe { msg_send![recognizer, recognitionTaskWithRequest: &*request, resultHandler: &*handler] };
    let task = task.ok_or_else(|| CaptureError::Speech("could not start recognition".into()))?;

    let (lock, cv) = &*pair;
    let mut slot = lock.lock().map_err(|_| CaptureError::Speech("lock poisoned".into()))?;
    let outcome = loop {
        if let Some(o) = slot.take() {
            break o;
        }
        if cancel.load(Ordering::Relaxed) {
            // SAFETY: `-cancel` on a live SFSpeechRecognitionTask.
            let () = unsafe { msg_send![&*task, cancel] };
            return Err(CaptureError::Cancelled);
        }
        slot = cv.wait_timeout(slot, POLL).map_err(|_| CaptureError::Speech("lock poisoned".into()))?.0;
    };
    drop(slot);
    match outcome {
        Ok(words) => Ok(words),
        Err(_) if cancel.load(Ordering::Relaxed) => Err(CaptureError::Cancelled),
        Err((domain, code, msg)) => map_speech_error(&domain, code, msg),
    }
}

/// Known `kAFAssistantErrorDomain` / `kLSRErrorDomain` codes.
fn map_speech_error(domain: &str, code: isize, msg: String) -> Result<Vec<Word>, CaptureError> {
    log::warn!("speech: {domain} {code}: {msg}");
    match (domain, code) {
        // "No speech detected": silence is not an error.
        ("kAFAssistantErrorDomain", 1110) => Ok(Vec::new()),
        ("kAFAssistantErrorDomain", 1700) | ("kLSRErrorDomain", 300) => Err(CaptureError::SpeechDenied),
        ("kAFAssistantErrorDomain", 1101 | 1107) | ("kLSRErrorDomain", 102 | 201) => {
            Err(CaptureError::SpeechUnavailable(msg))
        }
        ("kLSRErrorDomain", 301) | ("kAFAssistantErrorDomain", 216) => Err(CaptureError::Cancelled),
        _ => Err(CaptureError::Speech(msg)),
    }
}

/// Transcribe `audio` (any AVFoundation-readable file; typically from
/// [`export_audio`]) with Apple Speech in `locale` (e.g. `"en-US"`,
/// `"zh-CN"`). Uses on-device recognition whenever the recognizer supports
/// it for this locale. Audio longer than [`CHUNK_THRESHOLD_SECS`] is split
/// into [`CHUNK_SECS`] pieces and times are offset back. Returns grouped,
/// un-normalised segments (see [`crate::words::group_words`]). Blocks;
/// setting `cancel` stops it with [`CaptureError::Cancelled`].
pub fn transcribe_on_device(audio: &Path, locale: &str, cancel: Arc<AtomicBool>) -> Result<Vec<Segment>, CaptureError> {
    match speech_authorization() {
        SpeechAuthorization::Authorized => {}
        SpeechAuthorization::NotDetermined | SpeechAuthorization::Denied => return Err(CaptureError::SpeechDenied),
        SpeechAuthorization::Restricted => {
            return Err(CaptureError::SpeechUnavailable("speech recognition is restricted on this Mac".into()))
        }
    }
    let rec_cls = speech_class(c"SFSpeechRecognizer")?;
    // SAFETY: SFSpeechRecognizer / NSLocale / NSOperationQueue setup.
    let (recognizer, on_device) = unsafe {
        let loc: Retained<AnyObject> =
            msg_send![class!(NSLocale), localeWithLocaleIdentifier: &*NSString::from_str(locale)];
        let alloc: Allocated<AnyObject> = msg_send![rec_cls, alloc];
        let recognizer: Option<Retained<AnyObject>> = msg_send![alloc, initWithLocale: &*loc];
        let recognizer = recognizer
            .ok_or_else(|| CaptureError::SpeechUnavailable(format!("language “{locale}” is not supported")))?;
        let available: bool = msg_send![&*recognizer, isAvailable];
        if !available {
            return Err(CaptureError::SpeechUnavailable(format!("recognizer for “{locale}” is not available")));
        }
        let on_device: bool = msg_send![&*recognizer, supportsOnDeviceRecognition];
        // Deliver results on a private queue, not the main queue we may not run.
        let queue: Retained<AnyObject> = msg_send![class!(NSOperationQueue), new];
        let () = msg_send![&*recognizer, setQueue: &*queue];
        (recognizer, on_device)
    };
    if !on_device {
        log::warn!("speech: on-device recognition not supported for {locale}; Apple servers will be used");
    }

    let url = crate::clipboard::file_url(audio).ok_or_else(|| CaptureError::Framework("invalid audio path".into()))?;
    // SAFETY: synchronous duration read on a local file (worker thread).
    let (asset, duration) = unsafe {
        let asset = AVAsset::assetWithURL(&url);
        let d = asset.duration().seconds();
        (asset, d)
    };
    if !duration.is_finite() || duration <= CHUNK_THRESHOLD_SECS {
        let words = recognize_file(&recognizer, audio, on_device, &cancel)?;
        return Ok(group_words(&words, 0.0));
    }

    let mut out = Vec::new();
    let mut t = 0.0;
    while t < duration {
        if cancel.load(Ordering::Relaxed) {
            return Err(CaptureError::Cancelled);
        }
        let len = CHUNK_SECS.min(duration - t);
        // SAFETY: CMTime constructors.
        let range =
            unsafe { CMTimeRange { start: CMTime::with_seconds(t, 600), duration: CMTime::with_seconds(len, 600) } };
        let chunk = export_m4a(&asset, Some(range), "chunk", Some(&cancel))?;
        let words = recognize_file(&recognizer, &chunk, on_device, &cancel);
        let _ = std::fs::remove_file(&chunk);
        out.extend(group_words(&words?, t));
        t += CHUNK_SECS;
    }
    Ok(out)
}
