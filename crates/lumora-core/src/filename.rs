//! Output file naming from user templates.
//!
//! Tokens (case-sensitive):
//!
//! | token      | example      |
//! |------------|--------------|
//! | `{date}`   | `2026-11-15` |
//! | `{time}`   | `04.30.12`   |
//! | `{YYYY}` `{MM}` `{DD}` | `2026` `11` `15` |
//! | `{hh}` `{mm}` `{ss}`   | `04` `30` `12`   |
//! | `{kind}`   | `Screenshot` / `Recording` |
//!
//! The default template yields `Lumora 2026-11-15 at 04.30.12.png`, mirroring
//! macOS's own naming (dots in the time because `:` is not allowed in Finder).
//! The date/time parts are passed in by the caller (the app reads them from
//! the system calendar in the user's time zone), keeping this module pure.

use std::path::{Path, PathBuf};

pub const DEFAULT_TEMPLATE: &str = "Lumora {date} at {time}";

/// Local wall-clock time components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTimeParts {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// What kind of capture is being named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Screenshot,
    Recording,
}

impl CaptureKind {
    pub fn extension(self) -> &'static str {
        match self {
            CaptureKind::Screenshot => "png",
            CaptureKind::Recording => "mp4",
        }
    }
    fn word(self) -> &'static str {
        match self {
            CaptureKind::Screenshot => "Screenshot",
            CaptureKind::Recording => "Recording",
        }
    }
}

/// Expand `template` into a file stem (no extension). The result is
/// sanitised for Finder: `/` and `:` become `-`, control characters are
/// dropped, and leading dots are removed so files are never hidden.
pub fn render(template: &str, t: &DateTimeParts, kind: CaptureKind) -> String {
    let date = format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
    let time = format!("{:02}.{:02}.{:02}", t.hour, t.minute, t.second);
    let out = template
        .replace("{date}", &date)
        .replace("{time}", &time)
        .replace("{YYYY}", &format!("{:04}", t.year))
        .replace("{MM}", &format!("{:02}", t.month))
        .replace("{DD}", &format!("{:02}", t.day))
        .replace("{hh}", &format!("{:02}", t.hour))
        .replace("{mm}", &format!("{:02}", t.minute))
        .replace("{ss}", &format!("{:02}", t.second))
        .replace("{kind}", kind.word());
    let cleaned: String =
        out.chars().filter(|c| !c.is_control()).map(|c| if c == '/' || c == ':' { '-' } else { c }).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    // Finder limits names to 255 bytes; leave room for " (999).png".
    let mut s = cleaned;
    while s.len() > 240 {
        s.pop();
    }
    if s.is_empty() {
        render(DEFAULT_TEMPLATE, t, kind)
    } else {
        s
    }
}

/// Build a path in `dir` that does not exist yet, appending ` (2)`, ` (3)`…
/// when needed. `exists` is injected for testability; pass
/// `|p| p.exists()` in production.
pub fn unique_path(dir: &Path, stem: &str, ext: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !exists(&first) {
        return first;
    }
    (2..10_000).map(|n| dir.join(format!("{stem} ({n}).{ext}"))).find(|p| !exists(p)).unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: DateTimeParts = DateTimeParts { year: 2026, month: 11, day: 15, hour: 4, minute: 30, second: 12 };

    #[test]
    fn default_template_matches_spec() {
        assert_eq!(render(DEFAULT_TEMPLATE, &T, CaptureKind::Screenshot), "Lumora 2026-11-15 at 04.30.12");
    }

    #[test]
    fn all_tokens() {
        assert_eq!(
            render("{kind}_{YYYY}{MM}{DD}-{hh}{mm}{ss}", &T, CaptureKind::Recording),
            "Recording_20261115-043012"
        );
    }

    #[test]
    fn sanitises() {
        assert_eq!(render("..a/b:c\n", &T, CaptureKind::Screenshot), "a-b-c");
        assert_eq!(render("...", &T, CaptureKind::Screenshot), "Lumora 2026-11-15 at 04.30.12");
        assert!(render(&"x".repeat(500), &T, CaptureKind::Screenshot).len() <= 240);
    }

    #[test]
    fn unique_paths() {
        let dir = Path::new("/caps");
        let taken = [PathBuf::from("/caps/a.png"), PathBuf::from("/caps/a (2).png")];
        let p = unique_path(dir, "a", "png", |p| taken.iter().any(|t| t == p));
        assert_eq!(p, PathBuf::from("/caps/a (3).png"));
        assert_eq!(unique_path(dir, "b", "mp4", |_| false), PathBuf::from("/caps/b.mp4"));
        assert_eq!(CaptureKind::Recording.extension(), "mp4");
    }
}
