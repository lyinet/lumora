use thiserror::Error;

/// Errors surfaced by capture and recording.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum CaptureError {
    /// The user has not granted Screen Recording permission.
    #[error("Lumora needs Screen Recording permission")]
    PermissionDenied,
    /// The user has not granted Microphone permission (recording continues
    /// without the mic).
    #[error("Lumora needs Microphone permission")]
    MicrophoneDenied,
    /// The requested display or window no longer exists.
    #[error("the {0} to capture is no longer available")]
    TargetGone(&'static str),
    /// A framework call failed; carries `NSError.localizedDescription`.
    #[error("{0}")]
    Framework(String),
    /// Encoding or writing a file failed.
    #[error("could not write {path}: {reason}")]
    Write { path: String, reason: String },
    /// The selection was empty after clipping.
    #[error("nothing to capture")]
    Empty,
    /// The user has not granted Speech Recognition permission.
    #[error("Lumora needs Speech Recognition permission")]
    SpeechDenied,
    /// Speech recognition is not available (unsupported locale, Siri &
    /// Dictation disabled, or the recognizer is temporarily unavailable).
    #[error("speech recognition is unavailable: {0}")]
    SpeechUnavailable(String),
    /// The recognizer failed; carries `NSError.localizedDescription`.
    #[error("speech recognition failed: {0}")]
    Speech(String),
    /// The movie has no audio track to export or transcribe.
    #[error("the recording has no audio")]
    NoAudio,
    /// The operation was cancelled by the caller.
    #[error("cancelled")]
    Cancelled,
}
