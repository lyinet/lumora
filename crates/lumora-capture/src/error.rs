use thiserror::Error;

/// Errors surfaced by capture and recording.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum CaptureError {
    /// The user has not granted Screen Recording permission.
    #[error("Lumora needs Screen Recording permission")]
    PermissionDenied,
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
}
