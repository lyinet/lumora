//! Housekeeping for Lumora's scratch folder (`$TMPDIR/Lumora`), where
//! captures and recordings land when "Save to disk" is off.
//!
//! Only ever pointed at that folder. Entries are judged at the top level: a
//! file by its own mtime, a folder (a recording's `.tutorial/`) by the newest
//! mtime anywhere inside it, so a folder still being written to survives.
//! Symlinks are never followed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long scratch files are kept.
pub const TEMP_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Newest modification time of `path` (recursively for real directories).
fn newest_mtime(path: &Path) -> Option<SystemTime> {
    let meta = fs::symlink_metadata(path).ok()?;
    let own = meta.modified().ok()?;
    if !meta.is_dir() {
        return Some(own);
    }
    let mut newest = own;
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            if let Some(t) = newest_mtime(&e.path()) {
                newest = newest.max(t);
            }
        }
    }
    Some(newest)
}

/// Top-level entries of `dir` not modified within `max_age` of `now`.
pub fn stale_entries(dir: &Path, now: SystemTime, max_age: Duration) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| newest_mtime(p).and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > max_age))
        .collect();
    out.sort();
    out
}

/// Delete [`stale_entries`] of `dir`; returns how many were removed.
/// Refuses to run when `dir` overlaps any of `protected` (e.g. the user's
/// save folder pointed inside the scratch folder, or the other way round).
pub fn remove_stale(dir: &Path, now: SystemTime, max_age: Duration, protected: &[PathBuf]) -> usize {
    // Compare resolved paths too (`/var` vs `/private/var` on macOS).
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let overlaps = |a: &Path, b: &Path| a.starts_with(b) || b.starts_with(a);
    if protected.iter().any(|p| overlaps(p, dir) || overlaps(&canon(p), &canon(dir))) {
        return 0;
    }
    let mut removed = 0;
    for p in stale_entries(dir, now, max_age) {
        let is_dir = fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir());
        let r = if is_dir { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
        if r.is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumora-cleanup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path, t: SystemTime) {
        File::create(p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn removes_only_old_entries_and_judges_folders_by_newest_file() {
        let d = scratch("stale");
        let now = SystemTime::now();
        let old = now - Duration::from_secs(8 * 24 * 3600);
        touch(&d.join("old.png"), old);
        touch(&d.join("new.png"), now);
        let fresh_dir = d.join("Rec 1.tutorial");
        fs::create_dir(&fresh_dir).unwrap();
        touch(&fresh_dir.join("step-01.png"), old);
        touch(&fresh_dir.join("step-02.png"), now);
        let old_dir = d.join("Rec 0.tutorial");
        fs::create_dir(&old_dir).unwrap();
        touch(&old_dir.join("step-01.png"), old);
        // Directory mtimes are "now" after creating files; set them old.
        File::open(&old_dir).unwrap().set_modified(old).unwrap();
        File::open(&fresh_dir).unwrap().set_modified(old).unwrap();

        assert_eq!(stale_entries(&d, now, TEMP_MAX_AGE), vec![old_dir.clone(), d.join("old.png")]);
        // Protected overlap: nothing happens.
        assert_eq!(remove_stale(&d, now, TEMP_MAX_AGE, &[d.join("sub")]), 0);
        assert!(d.join("old.png").exists());
        assert_eq!(remove_stale(&d, now, TEMP_MAX_AGE, &[PathBuf::from("/Users/x/Desktop")]), 2);
        assert!(!old_dir.exists() && !d.join("old.png").exists());
        assert!(d.join("new.png").exists() && fresh_dir.join("step-01.png").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_dir_is_harmless() {
        let p = std::env::temp_dir().join("lumora-cleanup-does-not-exist-xyz");
        assert!(stale_entries(&p, SystemTime::now(), TEMP_MAX_AGE).is_empty());
    }
}
