//! # lumora-ai
//!
//! The I/O half of Lumora's AI writing: a small blocking client for
//! OpenAI-compatible chat-completion APIs (streaming, cancellation, friendly
//! errors), cloud transcription via `/audio/transcriptions`, and, on macOS, API-key storage in the Keychain.
//!
//! Prompt building, response parsing, settings and error types live in
//! [`lumora_core::ai`] and are re-exported here. Everything in [`client`]
//! blocks, so call it from a background GCD queue and hop back to the main
//! queue to touch AppKit (deltas included).

pub mod client;
#[cfg(target_os = "macos")]
pub mod keychain;
pub mod multipart;
pub mod transcribe;

pub use client::{complete, test_connection, ConnectionInfo};
#[cfg(target_os = "macos")]
pub use keychain::Keychain;
pub use lumora_core::ai::*;
pub use multipart::{random_boundary, Multipart};
pub use transcribe::{
    parse_verbose_json, transcribe, transcriptions_url, TranscribeError, TranscribeRequest, MAX_UPLOAD_BYTES,
};
