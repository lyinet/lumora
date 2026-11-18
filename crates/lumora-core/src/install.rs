//! Where the app bundle is running from.
//!
//! Gatekeeper runs a quarantined app that was not moved by Finder from a
//! random read-only mirror ("App Translocation",
//! `/private/var/folders/…/AppTranslocation/<uuid>/d/Lumora.app`). An app
//! started straight from the disk image (`/Volumes/Lumora 0.6.1/…`) is in a
//! similar spot. In both cases macOS privacy permissions (TCC) may not stick
//! to the app the user later finds in /Applications, so the Screen Recording
//! grant appears to "not work". Lumora detects both and offers to move itself.

use std::path::{Path, PathBuf};

/// Why the current bundle location is a problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationProblem {
    /// Running from Gatekeeper's App Translocation mirror.
    Translocated,
    /// Running from a mounted volume under `/Volumes` (usually the DMG).
    DiskImage,
}

/// Classify a bundle path (`NSBundle.mainBundle.bundlePath`). `None` for a
/// normal location, and for anything that is not an `.app` bundle (e.g. a
/// development binary run from `target/`).
pub fn location_problem(bundle_path: &str) -> Option<LocationProblem> {
    if !bundle_path.trim_end_matches('/').ends_with(".app") {
        return None;
    }
    if bundle_path.contains("/AppTranslocation/") {
        Some(LocationProblem::Translocated)
    } else if bundle_path.starts_with("/Volumes/") {
        Some(LocationProblem::DiskImage)
    } else {
        None
    }
}

/// Where a bundle should be copied: `/Applications/<bundle name>.app`.
pub fn applications_destination(bundle_path: &str) -> Option<PathBuf> {
    let name = Path::new(bundle_path.trim_end_matches('/')).file_name()?;
    Some(Path::new("/Applications").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_locations() {
        let t = "/private/var/folders/x1/abc/T/AppTranslocation/1B2C-33/d/Lumora.app";
        assert_eq!(location_problem(t), Some(LocationProblem::Translocated));
        assert_eq!(location_problem("/Volumes/Lumora 0.6.1/Lumora.app"), Some(LocationProblem::DiskImage));
        assert_eq!(location_problem("/Volumes/Lumora/Lumora.app/"), Some(LocationProblem::DiskImage));
        assert_eq!(location_problem("/Applications/Lumora.app"), None);
        assert_eq!(location_problem("/Users/me/Downloads/Lumora.app"), None);
        // Development binaries are never relocated.
        assert_eq!(location_problem("/Volumes/dev/lumora/target/release"), None);
    }

    #[test]
    fn destination_is_in_applications() {
        let t = "/private/var/folders/x/T/AppTranslocation/1/d/Lumora.app";
        assert_eq!(applications_destination(t), Some(PathBuf::from("/Applications/Lumora.app")));
        assert_eq!(applications_destination("/Volumes/L/Lumora.app/"), Some(PathBuf::from("/Applications/Lumora.app")));
    }
}
