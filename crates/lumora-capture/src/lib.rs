//! # lumora-capture
//!
//! Thin, safe(r) wrappers over ScreenCaptureKit, Core Graphics, ImageIO and
//! AVFoundation:
//!
//! * [`content`] — enumerate displays and windows (`SCShareableContent`),
//!   front-to-back ordered for hover picking.
//! * [`screenshot`] — still captures via `SCScreenshotManager`.
//! * [`image`] — cropping, pixel access for the loupe, PNG encoding with
//!   Retina DPI metadata.
//! * [`beautify_render`] — draw a capture on a beautify canvas
//!   (background, shadow, rounded frame, window chrome).
//! * [`clipboard`] — put a capture on the pasteboard.
//! * [`recorder`] — `SCStream` → `AVAssetWriter` MP4 recording.
//! * [`ocr`] — on-device text recognition (Vision).
//! * [`permission`] — Screen Recording (TCC) preflight / request.
//!
//! All asynchronous APIs take `Send + 'static` completion closures which are
//! invoked on an arbitrary queue; callers hop to the main thread themselves.
//! Nothing in this crate touches AppKit views.

mod error;
pub use error::CaptureError;

#[cfg(target_os = "macos")]
mod send;
#[cfg(target_os = "macos")]
pub use send::SendBox;

#[cfg(target_os = "macos")]
pub mod beautify_render;
#[cfg(target_os = "macos")]
pub mod clipboard;
#[cfg(target_os = "macos")]
pub mod content;
#[cfg(target_os = "macos")]
pub mod image;
#[cfg(target_os = "macos")]
pub mod ocr;
#[cfg(target_os = "macos")]
pub mod permission;
#[cfg(target_os = "macos")]
pub mod recorder;
#[cfg(target_os = "macos")]
pub mod screenshot;
