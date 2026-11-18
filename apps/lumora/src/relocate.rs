//! Offer to move Lumora out of App Translocation or the disk image.
//!
//! See [`lumora_core::install`] for why that matters: privacy permissions
//! granted to a translocated or DMG copy don't reliably stick, which shows up
//! as "Grant Access does nothing" or "Lumora is not in the list".

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use lumora_core::i18n::{tr, trf, Msg};
use lumora_core::install::{applications_destination, location_problem, LocationProblem};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication};
use objc2_foundation::{NSBundle, NSFileManager, NSString};

extern "C" {
    // <sys/xattr.h>
    fn removexattr(path: *const std::ffi::c_char, name: *const std::ffi::c_char, options: std::ffi::c_int) -> i32;
}
const XATTR_NOFOLLOW: std::ffi::c_int = 0x0001;

/// Check where we run from; when it is a problem, ask to move. Returns
/// `true` when a relocated copy was launched and this process is quitting
/// (the caller must not start the app).
pub fn check(mtm: MainThreadMarker) -> bool {
    let bundle = NSBundle::mainBundle().bundlePath().to_string();
    let Some(problem) = location_problem(&bundle) else { return false };
    log::warn!("running from {bundle} ({problem:?})");
    let Some(dest) = applications_destination(&bundle) else { return false };

    NSApplication::sharedApplication(mtm).activate();
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(tr(Msg::LocationTitle)));
    let body = match problem {
        LocationProblem::Translocated => tr(Msg::LocationTranslocated),
        LocationProblem::DiskImage => tr(Msg::LocationDiskImage),
    };
    alert.setInformativeText(&NSString::from_str(body));
    alert.addButtonWithTitle(&NSString::from_str(tr(Msg::MoveToApplications)));
    alert.addButtonWithTitle(&NSString::from_str(tr(Msg::NotNow)));
    if alert.runModal() != NSAlertFirstButtonReturn {
        log::info!("user kept the current location");
        return false;
    }
    match move_to(Path::new(&bundle), &dest) {
        Ok(()) => {
            log::info!("copied to {}; relaunching", dest.display());
            match std::process::Command::new("/usr/bin/open").arg("-n").arg(&dest).spawn() {
                Ok(_) => {
                    NSApplication::sharedApplication(mtm).terminate(None);
                    true
                }
                Err(e) => {
                    log::error!("relaunch from {} failed: {e}", dest.display());
                    false
                }
            }
        }
        Err(e) => {
            log::error!("moving to {} failed: {e}", dest.display());
            lumora_ui::util::warning_alert(
                mtm,
                tr(Msg::LocationMoveFailed),
                &trf(Msg::LocationMoveFailedDetail, &[&e]),
            );
            false
        }
    }
}

/// Copy the bundle to `dest` (an older copy there goes to the Trash first)
/// and strip the quarantine flag from the copy so it is not translocated
/// again.
fn move_to(src: &Path, dest: &Path) -> Result<(), String> {
    let fm = NSFileManager::defaultManager();
    let url = |p: &Path| lumora_capture::clipboard::file_url(p).ok_or_else(|| format!("bad path {}", p.display()));
    let (src_url, dest_url) = (url(src)?, url(dest)?);
    if dest.exists() {
        fm.trashItemAtURL_resultingItemURL_error(&dest_url, None).map_err(|e| e.localizedDescription().to_string())?;
    }
    fm.copyItemAtURL_toURL_error(&src_url, &dest_url).map_err(|e| e.localizedDescription().to_string())?;
    let n = strip_quarantine(dest);
    log::info!("removed quarantine from {n} item(s)");
    Ok(())
}

/// Remove `com.apple.quarantine` from `root` and everything below it
/// (symlinks themselves, never their targets). Returns how many items had it.
fn strip_quarantine(root: &Path) -> usize {
    let name = c"com.apple.quarantine";
    let mut count = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        if let Ok(c) = CString::new(p.as_os_str().as_bytes()) {
            // SAFETY: two valid NUL-terminated strings.
            if unsafe { removexattr(c.as_ptr(), name.as_ptr(), XATTR_NOFOLLOW) } == 0 {
                count += 1;
            }
        }
        let is_dir = std::fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        }
    }
    count
}
