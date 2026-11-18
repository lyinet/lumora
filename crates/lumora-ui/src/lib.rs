//! # lumora-ui
//!
//! Native AppKit + Core Animation interface for Lumora. No web views: every
//! surface is an `NSPanel`/`NSWindow` hosting layer trees, and every motion is
//! a `CASpringAnimation` (or a frame-driven [`lumora_core::spring`] solver
//! for window frames, which Core Animation cannot animate).
//!
//! Components never call back into the app directly. They emit
//! [`events::UiEvent`]s into an [`events::EventSink`], which the app drains
//! asynchronously on the main queue — this avoids re-entrancy (a component
//! being torn down from inside its own callback).
//!
//! [`tokens`] (design tokens) is platform independent and unit-tested on
//! every host; everything else is macOS-only.

pub mod tokens;

#[cfg(target_os = "macos")]
mod ai_sheet;
#[cfg(target_os = "macos")]
pub mod anim;
#[cfg(target_os = "macos")]
pub mod click_overlay;
#[cfg(target_os = "macos")]
pub mod countdown;
#[cfg(target_os = "macos")]
pub mod editor;
#[cfg(target_os = "macos")]
mod esc_window;
#[cfg(target_os = "macos")]
pub mod events;
#[cfg(target_os = "macos")]
pub mod history_window;
#[cfg(target_os = "macos")]
pub mod icon;
#[cfg(target_os = "macos")]
pub mod keystroke_hud;
#[cfg(target_os = "macos")]
pub mod onboarding;
#[cfg(target_os = "macos")]
pub mod overlay;
#[cfg(target_os = "macos")]
pub mod pin;
#[cfg(target_os = "macos")]
pub mod quick_access;
#[cfg(target_os = "macos")]
pub mod recording_hud;
#[cfg(target_os = "macos")]
pub mod settings_window;
#[cfg(target_os = "macos")]
pub mod shutter;
#[cfg(target_os = "macos")]
pub mod text_result;
#[cfg(target_os = "macos")]
pub mod toast;
#[cfg(target_os = "macos")]
pub mod transcript_window;
#[cfg(target_os = "macos")]
pub mod trim_window;
#[cfg(target_os = "macos")]
pub mod tutorial_window;
#[cfg(target_os = "macos")]
pub mod util;
