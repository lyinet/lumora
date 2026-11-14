//! User settings, persisted as TOML in
//! `~/Library/Application Support/Lumora/settings.toml`.
//!
//! Every field has a serde default, so files written by older (or newer)
//! versions load cleanly: unknown keys are ignored and missing keys fall back
//! to defaults. Values are validated/clamped after loading so a hand-edited
//! file can never put the app into a broken state.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::action::Action;
use crate::hotkey::Hotkey;

/// Errors from loading or saving settings.
#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("could not read or write settings: {0}")]
    Io(#[from] std::io::Error),
    #[error("settings file is not valid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not serialise settings: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: General,
    pub shortcuts: Shortcuts,
    pub capture: Capture,
    pub recording: Recording,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Where captures are saved. `None` means `~/Desktop`.
    pub save_folder: Option<PathBuf>,
    /// Filename template; see [`crate::filename`].
    pub filename_template: String,
    pub copy_to_clipboard: bool,
    pub save_to_disk: bool,
    pub show_quick_access: bool,
    /// Seconds before an un-hovered Quick Access card dismisses itself.
    pub quick_access_timeout: f64,
    pub play_sounds: bool,
    pub launch_at_login: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub capture_area: Option<Hotkey>,
    pub capture_fullscreen: Option<Hotkey>,
    pub capture_window: Option<Hotkey>,
    pub record_screen: Option<Hotkey>,
    pub open_folder: Option<Hotkey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    pub show_magnifier: bool,
    pub show_crosshair: bool,
    /// Include the window's drop shadow in window captures.
    pub window_shadow: bool,
    /// Hover-to-pick windows in the area overlay.
    pub window_picking: bool,
    /// Save at the display's native (Retina) scale rather than 1×.
    pub retina: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    H264,
    Hevc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recording {
    /// Frames per second: 30 or 60.
    pub fps: u32,
    pub codec: Codec,
    pub show_cursor: bool,
    pub system_audio: bool,
    pub microphone: bool,
    /// Countdown before recording starts; 0 disables it.
    pub countdown: u32,
    /// Target video bitrate in Mbit/s at 1080p; scaled by pixel count.
    pub quality_mbps: f64,
}

impl Default for General {
    fn default() -> Self {
        General {
            save_folder: None,
            filename_template: crate::filename::DEFAULT_TEMPLATE.to_string(),
            copy_to_clipboard: true,
            save_to_disk: true,
            show_quick_access: true,
            quick_access_timeout: 6.0,
            play_sounds: true,
            launch_at_login: false,
        }
    }
}

impl Default for Shortcuts {
    fn default() -> Self {
        let d = |a: Action| a.default_shortcut().parse().ok();
        Shortcuts {
            capture_area: d(Action::CaptureArea),
            capture_fullscreen: d(Action::CaptureFullscreen),
            capture_window: d(Action::CaptureWindow),
            record_screen: d(Action::RecordScreen),
            open_folder: d(Action::OpenFolder),
        }
    }
}

impl Shortcuts {
    pub fn get(&self, a: Action) -> Option<Hotkey> {
        match a {
            Action::CaptureArea => self.capture_area,
            Action::CaptureFullscreen => self.capture_fullscreen,
            Action::CaptureWindow => self.capture_window,
            Action::RecordScreen => self.record_screen,
            Action::OpenFolder => self.open_folder,
        }
    }

    pub fn set(&mut self, a: Action, h: Option<Hotkey>) {
        let slot = match a {
            Action::CaptureArea => &mut self.capture_area,
            Action::CaptureFullscreen => &mut self.capture_fullscreen,
            Action::CaptureWindow => &mut self.capture_window,
            Action::RecordScreen => &mut self.record_screen,
            Action::OpenFolder => &mut self.open_folder,
        };
        *slot = h;
    }

    /// All `(action, hotkey)` pairs that are bound.
    pub fn bound(&self) -> Vec<(Action, Hotkey)> {
        Action::ALL.iter().filter_map(|&a| self.get(a).map(|h| (a, h))).collect()
    }
}

impl Default for Capture {
    fn default() -> Self {
        Capture { show_magnifier: true, show_crosshair: true, window_shadow: true, window_picking: true, retina: true }
    }
}

impl Default for Recording {
    fn default() -> Self {
        Recording {
            fps: 60,
            codec: Codec::H264,
            show_cursor: true,
            system_audio: false,
            microphone: false,
            countdown: 3,
            quality_mbps: 12.0,
        }
    }
}

impl Settings {
    /// `~/Library/Application Support/Lumora/settings.toml`, derived from
    /// `$HOME`. Returns `None` if `$HOME` is unset.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| Self::path_in_home(Path::new(&h)))
    }

    /// The settings path for a given home directory.
    pub fn path_in_home(home: &Path) -> PathBuf {
        home.join("Library/Application Support/Lumora/settings.toml")
    }

    /// Parse from TOML and validate.
    pub fn from_toml(s: &str) -> Result<Settings, SettingsError> {
        let mut settings: Settings = toml::from_str(s)?;
        settings.validate();
        Ok(settings)
    }

    pub fn to_toml(&self) -> Result<String, SettingsError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load from `path`. A missing file yields defaults; a corrupt file is an
    /// error (the caller should back it up and fall back to defaults).
    pub fn load(path: &Path) -> Result<Settings, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_toml(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Load, falling back to defaults on any error. A corrupt file is moved
    /// aside to `settings.toml.bak` so the user's data is not lost.
    pub fn load_or_default(path: &Path) -> Settings {
        match Self::load(path) {
            Ok(s) => s,
            Err(SettingsError::Parse(_)) => {
                let _ = std::fs::rename(path, path.with_extension("toml.bak"));
                Settings::default()
            }
            Err(_) => Settings::default(),
        }
    }

    /// Atomically write to `path` (write temp file, then rename), creating
    /// the parent directory if needed.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml()?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Clamp values into supported ranges.
    pub fn validate(&mut self) {
        let r = &mut self.recording;
        r.fps = if r.fps >= 45 { 60 } else { 30 };
        r.countdown = r.countdown.min(10);
        if !r.quality_mbps.is_finite() {
            r.quality_mbps = 12.0;
        }
        r.quality_mbps = r.quality_mbps.clamp(1.0, 100.0);
        let g = &mut self.general;
        if !g.quick_access_timeout.is_finite() {
            g.quick_access_timeout = 6.0;
        }
        g.quick_access_timeout = g.quick_access_timeout.clamp(1.0, 600.0);
        if g.filename_template.trim().is_empty() {
            g.filename_template = crate::filename::DEFAULT_TEMPLATE.to_string();
        }
    }

    /// Resolved save folder for a given home directory.
    pub fn save_folder(&self, home: &Path) -> PathBuf {
        self.general.save_folder.clone().unwrap_or_else(|| home.join("Desktop"))
    }
}

impl Recording {
    /// Average bitrate (bits/s) for an output of `width`×`height` pixels,
    /// scaled from the 1080p reference and by frame rate.
    pub fn bitrate_for(&self, width: u32, height: u32) -> u64 {
        let px = f64::from(width) * f64::from(height);
        let reference = 1920.0 * 1080.0;
        let fps_factor = if self.fps >= 60 { 1.5 } else { 1.0 };
        let codec_factor = match self.codec {
            Codec::H264 => 1.0,
            Codec::Hevc => 0.7, // HEVC needs ~30 % less for the same quality.
        };
        (self.quality_mbps * 1_000_000.0 * (px / reference).max(0.05) * fps_factor * codec_factor) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let s = Settings::default();
        let t = s.to_toml().unwrap();
        assert!(t.contains("[shortcuts]"));
        assert!(t.contains("capture_area = \"⌥⇧4\""));
        assert_eq!(Settings::from_toml(&t).unwrap(), s);
    }

    #[test]
    fn partial_files_fill_defaults_and_ignore_unknown() {
        let s = Settings::from_toml(
            r#"
            future_section = { x = 1 }
            [recording]
            fps = 30
            codec = "hevc"
            unknown_key = true
            "#,
        )
        .unwrap();
        assert_eq!(s.recording.fps, 30);
        assert_eq!(s.recording.codec, Codec::Hevc);
        assert!(s.recording.show_cursor);
        assert_eq!(s.general, General::default());
    }

    #[test]
    fn validation_clamps() {
        let s = Settings::from_toml(
            "[recording]\nfps = 144\ncountdown = 99\nquality_mbps = 0.0\n[general]\nquick_access_timeout = -3.0\nfilename_template = \"  \"\n",
        )
        .unwrap();
        assert_eq!(s.recording.fps, 60);
        assert_eq!(s.recording.countdown, 10);
        assert_eq!(s.recording.quality_mbps, 1.0);
        assert_eq!(s.general.quick_access_timeout, 1.0);
        assert_eq!(s.general.filename_template, crate::filename::DEFAULT_TEMPLATE);
    }

    #[test]
    fn bad_shortcut_is_a_parse_error() {
        assert!(Settings::from_toml("[shortcuts]\ncapture_area = \"shift+a\"").is_err());
    }

    #[test]
    fn shortcuts_accessors() {
        let mut s = Shortcuts::default();
        assert_eq!(s.bound().len(), 5);
        s.set(Action::OpenFolder, None);
        assert_eq!(s.get(Action::OpenFolder), None);
        assert_eq!(s.bound().len(), 4);
    }

    #[test]
    fn save_and_load_on_disk() {
        let dir = std::env::temp_dir().join(format!("lumora-settings-test-{}", std::process::id()));
        let path = Settings::path_in_home(&dir);
        assert!(Settings::load(&path).unwrap() == Settings::default(), "missing file → defaults");
        let mut s = Settings::default();
        s.general.save_folder = Some(PathBuf::from("/tmp/caps"));
        s.recording.microphone = true;
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        // Corrupt file is backed up and defaults returned.
        std::fs::write(&path, "this is = = not toml").unwrap();
        assert_eq!(Settings::load_or_default(&path), Settings::default());
        assert!(path.with_extension("toml.bak").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_folder_defaults_to_desktop() {
        let s = Settings::default();
        assert_eq!(s.save_folder(Path::new("/Users/a")), PathBuf::from("/Users/a/Desktop"));
    }

    #[test]
    fn bitrate_scales() {
        let r = Recording { fps: 30, ..Recording::default() };
        assert_eq!(r.bitrate_for(1920, 1080), 12_000_000);
        assert!(r.bitrate_for(3840, 2160) > r.bitrate_for(1920, 1080) * 3);
        let hevc = Recording { codec: Codec::Hevc, ..r.clone() };
        assert!(hevc.bitrate_for(1920, 1080) < r.bitrate_for(1920, 1080));
    }
}
