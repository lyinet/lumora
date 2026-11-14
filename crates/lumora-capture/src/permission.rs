//! Screen Recording permission (TCC).
//!
//! macOS gates all screen capture behind the "Screen & System Audio
//! Recording" privacy pane. `CGPreflightScreenCaptureAccess` checks silently;
//! `CGRequestScreenCaptureAccess` shows the system prompt the *first* time and
//! afterwards just returns the current state, so the onboarding panel also
//! offers a button that opens System Settings directly.

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

/// URL that opens System Settings at the Screen Recording pane.
pub const SCREEN_RECORDING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// URL that opens System Settings at the Microphone pane.
pub const MICROPHONE_SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
