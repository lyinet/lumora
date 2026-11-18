//! Screen Recording permission (TCC).
//!
//! macOS gates all screen capture behind the "Screen & System Audio
//! Recording" privacy pane. `CGPreflightScreenCaptureAccess` checks silently;
//! `CGRequestScreenCaptureAccess` shows the system prompt the *first* time and
//! afterwards just returns the current state, so the onboarding panel also
//! offers a button that opens System Settings directly.
//!
//! On macOS 15 and later `CGRequestScreenCaptureAccess` alone does not
//! always add the app to the privacy list (seen on macOS 26 with an ad-hoc
//! signed build): TCC only registers the app once it actually asks
//! ScreenCaptureKit for content. [`probe_screen_capture`] does exactly that.

use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

/// Whether the app currently has Screen Recording permission. Never prompts.
pub fn has_screen_recording() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Ask for permission (shows the system prompt on first call). Returns the
/// state at the time of the call — a grant only takes effect after relaunch.
pub fn request_screen_recording() -> bool {
    CGRequestScreenCaptureAccess()
}

/// Ask ScreenCaptureKit for shareable content once, purely so TCC registers
/// Lumora in System Settings → Privacy & Security → Screen & System Audio
/// Recording and shows the system prompt. `completion` gets whether the
/// query succeeded (i.e. access is effective for this process) and runs on
/// an arbitrary queue. Never panics; failures are only logged.
pub fn probe_screen_capture(completion: impl FnOnce(bool) + Send + 'static) {
    crate::content::ShareableContent::fetch(move |res| {
        match &res {
            Ok(_) => log::info!("screen capture probe: content available"),
            Err(e) => log::info!("screen capture probe: {e}"),
        }
        completion(res.is_ok());
    });
}

/// URL that opens System Settings at the Screen Recording pane.
pub const SCREEN_RECORDING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// URL that opens System Settings at the Microphone pane.
pub const MICROPHONE_SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
