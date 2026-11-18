//! Small OS integrations: local time, sounds, login item, Finder, files.

use std::path::{Path, PathBuf};

use lumora_core::filename::DateTimeParts;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSSound, NSWorkspace};
use objc2_foundation::{NSArray, NSBundle, NSCalendar, NSCalendarUnit, NSDate, NSFileManager, NSString, NSURL};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// Current local wall-clock time from the user's calendar and time zone.
pub fn now_parts() -> DateTimeParts {
    let cal = NSCalendar::currentCalendar();
    let now = NSDate::now();
    let c = |u: NSCalendarUnit| cal.component_fromDate(u, &now);
    DateTimeParts {
        year: c(NSCalendarUnit::Year) as i32,
        month: c(NSCalendarUnit::Month) as u32,
        day: c(NSCalendarUnit::Day) as u32,
        hour: c(NSCalendarUnit::Hour) as u32,
        minute: c(NSCalendarUnit::Minute) as u32,
        second: c(NSCalendarUnit::Second) as u32,
    }
}

/// The user's preferred languages, most preferred first
/// (`NSLocale.preferredLanguages`, e.g. `["zh-Hans-CN", "en-US"]`).
pub fn preferred_languages() -> Vec<String> {
    objc2_foundation::NSLocale::preferredLanguages().iter().map(|s| s.to_string()).collect()
}

/// Apply the language setting to the process-wide i18n state.
pub fn apply_language(pref: lumora_core::i18n::LanguagePref) -> lumora_core::i18n::Language {
    let lang = pref.resolve(&preferred_languages());
    lumora_core::i18n::set_language(lang);
    lang
}

/// Seconds since the Unix epoch (UTC).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Where captures go when "Save to disk" is off (they still need a file for
/// drag-and-drop). Pruned at launch by [`clean_temp_dir`].
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("Lumora")
}

/// On a background thread, delete scratch-folder entries untouched for
/// [`lumora_core::cleanup::TEMP_MAX_AGE`] (7 days). Only [`temp_dir`] is
/// touched, and nothing at all if it overlaps the user's save folder.
pub fn clean_temp_dir(save_folder: PathBuf) {
    let dir = temp_dir();
    let spawned = std::thread::Builder::new().name("lumora-temp-cleanup".into()).spawn(move || {
        let n = lumora_core::cleanup::remove_stale(
            &dir,
            std::time::SystemTime::now(),
            lumora_core::cleanup::TEMP_MAX_AGE,
            &[save_folder],
        );
        if n > 0 {
            log::info!("removed {n} old item(s) from {}", dir.display());
        }
    });
    if let Err(e) = spawned {
        log::warn!("temp cleanup not started: {e}");
    }
}

const SHUTTER: &str =
    "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/system/Screen Capture.aif";

/// The system screenshot shutter sound (falls back to "Tink").
pub fn play_shutter() {
    let sound = NSSound::initWithContentsOfFile_byReference(NSSound::alloc(), &NSString::from_str(SHUTTER), true)
        .or_else(|| NSSound::soundNamed(&NSString::from_str("Tink")));
    if let Some(s) = sound {
        s.play();
    }
}

/// Register/unregister the app as a login item (macOS 13+ `SMAppService`).
pub fn set_launch_at_login(enabled: bool) -> Result<(), String> {
    // SAFETY: SMAppService calls with no preconditions beyond running from a
    // bundle (they return an error otherwise).
    unsafe {
        let service = SMAppService::mainAppService();
        let registered = service.status() == SMAppServiceStatus::Enabled;
        let result = match (enabled, registered) {
            (true, false) => service.registerAndReturnError(),
            (false, true) => service.unregisterAndReturnError(),
            _ => Ok(()),
        };
        result.map_err(|e| e.localizedDescription().to_string())
    }
}

pub fn reveal_in_finder(path: &Path) {
    if let Some(url) = lumora_capture::clipboard::file_url(path) {
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    }
}

pub fn open_folder(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    if let Some(url) = lumora_capture::clipboard::file_url(path) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

pub fn open_url(s: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(s)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// Move a file or folder to the Trash. Never deletes: `false` when the
/// Trash refused it (another volume without a Trash, permissions…).
pub fn move_to_trash(path: &Path) -> bool {
    lumora_capture::clipboard::file_url(path)
        .is_some_and(|url| NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None).is_ok())
}

/// Move a file to the Trash (recoverable), falling back to deletion.
pub fn trash(path: &Path) {
    if let Some(url) = lumora_capture::clipboard::file_url(path) {
        if NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None).is_ok() {
            return;
        }
    }
    let _ = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
}

/// Start a fresh instance of the bundle and quit this one (needed after a
/// Screen Recording grant).
pub fn relaunch(mtm: MainThreadMarker) {
    let bundle = NSBundle::mainBundle().bundlePath().to_string();
    if bundle.ends_with(".app") {
        let _ = std::process::Command::new("/usr/bin/open").args(["-n", &bundle]).spawn();
    }
    NSApplication::sharedApplication(mtm).terminate(None);
}
