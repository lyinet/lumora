//! Recording → step-by-step tutorial.
//!
//! A recording's click log (`<stem>.events.json`, see [`crate::click_log`])
//! is turned into a list of [`Step`]s: one per meaningful click inside the
//! recorded area. The app grabs a keyframe just after each click, draws a
//! ring at the click, crops around it ([`crop_rect`]) and reads the text
//! next to the pointer ([`ocr_rect`], [`ocr_hint`]). Titles come from AI
//! ([`ai`]) or, without it, from the local templates in [`local_title`].
//!
//! The edited tutorial is saved as `<stem>.steps.json` ([`sidecar_path`],
//! schema [`SCHEMA_VERSION`]) and exported as Markdown or a single
//! self-contained HTML page ([`export`]).
//!
//! Everything here is pure; the macOS side only produces images and OCR.

pub mod ai;
pub mod export;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::click_log::{ClickEvent, EventLog, MouseButton};
use crate::geometry::{Point, Rect, Size};
use crate::i18n::{format_args_into, Language, Msg};
use crate::ocr::TextBox;

/// Current `steps.json` schema version.
pub const SCHEMA_VERSION: u32 = 1;

fn stem_of(movie: &Path) -> String {
    movie.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "recording".into())
}

/// `/dir/Rec 1.mp4` → `/dir/Rec 1.steps.json`.
pub fn sidecar_path(movie: &Path) -> PathBuf {
    movie.with_file_name(format!("{}.steps.json", stem_of(movie)))
}

/// Whether `p` is a non-empty relative path made only of normal components
/// (no `..`, `.`, root or prefix), so `base.join(p)` stays under `base`.
pub fn is_safe_relative(p: &str) -> bool {
    use std::path::Component;
    !p.is_empty() && !p.contains('\\') && Path::new(p).components().all(|c| matches!(c, Component::Normal(_)))
}

/// Folder for the step images: `/dir/Rec 1.mp4` → `/dir/Rec 1.tutorial`.
pub fn image_dir(movie: &Path) -> PathBuf {
    movie.with_file_name(format!("{}.tutorial", stem_of(movie)))
}

/// File name of step `index` (1-based) inside [`image_dir`].
pub fn image_file_name(index: usize) -> String {
    format!("step-{index:02}.png")
}

/// Whether `path` looks like a movie Lumora can make a tutorial from.
pub fn is_movie(path: &Path) -> bool {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).is_some_and(|e| e == "mp4" || e == "mov")
}

/// Tuning for [`build_steps`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuildOptions {
    /// Clicks this close in time (seconds) …
    pub merge_window: f64,
    /// … and space (video pixels) are one step (double clicks, jitter).
    pub merge_distance: f64,
    /// Any two steps closer than this (seconds) collapse into the first.
    pub min_gap: f64,
    /// At most this many steps (the first ones are kept).
    pub max_steps: usize,
    /// The keyframe is taken this long after the click, so the UI has
    /// reacted (button highlight, menu open).
    pub frame_delay: f64,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions { merge_window: 0.4, merge_distance: 24.0, min_gap: 0.15, max_steps: 40, frame_delay: 0.12 }
    }
}

/// One step of a tutorial.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// 1-based position (renumbered after deletes).
    pub index: usize,
    /// Click time, seconds since the first frame.
    pub t: f64,
    /// Time of the keyframe.
    pub frame_time: f64,
    /// `[x, y]` click in video pixels.
    pub click: [f64; 2],
    pub button: MouseButton,
    /// Double (or triple) click.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub double_click: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    /// Text recognised right at / next to the click.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_hint: Option<String>,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// Image path relative to the `.steps.json` file's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// `[x, y, w, h]` video-pixel crop the image shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<[f64; 4]>,
}

impl Step {
    pub fn click_point(&self) -> Point {
        Point::new(self.click[0], self.click[1])
    }
}

/// The whole `steps.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tutorial {
    pub schema_version: u32,
    /// Movie file name (no directory).
    pub movie: String,
    #[serde(default)]
    pub title: String,
    pub video_width: u32,
    pub video_height: u32,
    /// Pixels per point of the recording.
    #[serde(default = "one")]
    pub scale: f64,
    /// Whether the titles were written by AI.
    #[serde(default)]
    pub ai_written: bool,
    pub steps: Vec<Step>,
}

fn one() -> f64 {
    1.0
}

impl Tutorial {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Parse a `steps.json`. Step images that would escape the file's folder
    /// (absolute, `..`, a drive/root prefix) are dropped, since export reads
    /// and embeds whatever they point at.
    pub fn from_json(s: &str) -> Result<Tutorial, serde_json::Error> {
        let mut t: Tutorial = serde_json::from_str(s)?;
        t.sanitize();
        Ok(t)
    }

    /// Drop every step image path that is not a plain relative path.
    pub fn sanitize(&mut self) {
        for step in &mut self.steps {
            if step.image.as_deref().is_some_and(|p| !is_safe_relative(p)) {
                step.image = None;
            }
        }
    }

    /// The movie moved from `from` to `to` (e.g. Save out of the scratch
    /// folder, possibly with a new unique name): point `movie` at the new file
    /// name and step images under the old `<stem>.tutorial/` folder at the
    /// new one. Returns whether anything changed.
    pub fn relocate(&mut self, from: &Path, to: &Path) -> bool {
        let dir_name =
            |m: &Path| image_dir(m).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (old, new) = (format!("{}/", dir_name(from)), format!("{}/", dir_name(to)));
        let movie = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut changed = self.movie != movie;
        self.movie = movie;
        if old != new {
            for img in self.steps.iter_mut().filter_map(|s| s.image.as_mut()) {
                if let Some(rest) = img.strip_prefix(&old) {
                    *img = format!("{new}{rest}");
                    changed = true;
                }
            }
        }
        changed
    }

    /// Remove step `index` (1-based) and renumber.
    pub fn remove_step(&mut self, index: usize) {
        self.steps.retain(|s| s.index != index);
        self.renumber();
    }

    pub fn renumber(&mut self) {
        for (i, s) in self.steps.iter_mut().enumerate() {
            s.index = i + 1;
        }
    }

    /// Fill every step's title from [`local_title`] (detail cleared).
    pub fn apply_local_titles(&mut self, lang: Language) {
        for s in &mut self.steps {
            s.title = local_title(s, lang);
            s.detail = local_detail(s, lang);
        }
        self.ai_written = false;
    }
}

/// Turn a click log into steps (titles empty; see [`local_title`]).
/// Returns the steps and how many were dropped by the cap.
pub fn build_steps(log: &EventLog, opts: &BuildOptions) -> (Vec<Step>, usize) {
    let duration = log.recording.duration;
    let mut clicks: Vec<&ClickEvent> =
        log.clicks.iter().filter(|c| c.inside_recording && c.video_pixel.is_some() && c.t.is_finite()).collect();
    clicks.sort_by(|a, b| a.t.total_cmp(&b.t));

    let mut steps: Vec<Step> = Vec::new();
    // Time of the previous kept-or-merged click.
    let mut prev_t = f64::NEG_INFINITY;
    for c in clicks {
        let [x, y] = c.video_pixel.unwrap_or_default();
        let p = Point::new(x, y);
        let dt = c.t - prev_t;
        prev_t = c.t;
        if let Some(last) = steps.last_mut() {
            let near = p.distance(last.click_point()) <= opts.merge_distance;
            if (dt <= opts.merge_window && near) || dt < opts.min_gap {
                // Same gesture; a second click on the same spot makes it a double click.
                if near && c.button == last.button && c.click_count >= 2 {
                    last.double_click = true;
                }
                // The frame follows the last click of the gesture.
                last.frame_time = frame_time(c.t, duration, opts.frame_delay);
                continue;
            }
        }
        steps.push(Step {
            index: steps.len() + 1,
            t: c.t,
            frame_time: frame_time(c.t, duration, opts.frame_delay),
            click: [x, y],
            button: c.button,
            double_click: c.click_count >= 2,
            app: c.app.name.clone().filter(|s| !s.trim().is_empty()),
            bundle_id: c.app.bundle_id.clone(),
            window_title: c.app.window_title.clone().filter(|s| !s.trim().is_empty()),
            ocr_hint: None,
            title: String::new(),
            detail: String::new(),
            image: None,
            crop: None,
        });
    }
    let dropped = steps.len().saturating_sub(opts.max_steps);
    steps.truncate(opts.max_steps);
    (steps, dropped)
}

/// A new tutorial for `log` with local titles in `lang`.
pub fn tutorial_from_log(log: &EventLog, opts: &BuildOptions, lang: Language) -> (Tutorial, usize) {
    let (steps, dropped) = build_steps(log, opts);
    let movie_stem = Path::new(&log.recording.movie)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| log.recording.movie.clone());
    let mut t = Tutorial {
        schema_version: SCHEMA_VERSION,
        movie: log.recording.movie.clone(),
        title: movie_stem,
        video_width: log.recording.video_width,
        video_height: log.recording.video_height,
        scale: if log.recording.scale > 0.0 { log.recording.scale } else { 1.0 },
        ai_written: false,
        steps,
    };
    t.apply_local_titles(lang);
    (t, dropped)
}

fn frame_time(t: f64, duration: f64, delay: f64) -> f64 {
    let ft = t + delay;
    let ft = if duration > 0.0 { ft.min((duration - 0.04).max(t.min(duration))) } else { ft };
    (ft.max(0.0) * 1000.0).round() / 1000.0
}

/// An aspect-preserving crop of a `video`-sized frame around `center`:
/// at least `min_width` pixels wide (or half the frame, whichever is
/// larger), never larger than the frame, shifted to stay inside it.
pub fn crop_rect(center: Point, video: Size, min_width: f64) -> Rect {
    if video.width <= 0.0 || video.height <= 0.0 {
        return Rect::new(0.0, 0.0, 0.0, 0.0);
    }
    let w = (video.width * 0.5).max(min_width).min(video.width);
    let h = (w * video.height / video.width).min(video.height);
    let x = (center.x - w / 2.0).clamp(0.0, video.width - w);
    let y = (center.y - h / 2.0).clamp(0.0, video.height - h);
    Rect::new(x.round(), y.round(), w.round().min(video.width), h.round().min(video.height))
}

/// The region read for the OCR hint: a strip around the click,
/// `scale`-aware (≈ 360 × 72 points), clamped to the frame.
pub fn ocr_rect(center: Point, video: Size, scale: f64) -> Rect {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let w = (360.0 * s).min(video.width.max(0.0));
    let h = (72.0 * s).min(video.height.max(0.0));
    let x = (center.x - w / 2.0).clamp(0.0, (video.width - w).max(0.0));
    let y = (center.y - h / 2.0).clamp(0.0, (video.height - h).max(0.0));
    Rect::new(x.round(), y.round(), w.round(), h.round())
}

/// Longest hint kept, in characters.
pub const MAX_HINT_CHARS: usize = 48;

/// The text the user most likely clicked: the recognised box containing
/// `click` (both in the same pixel space), else the nearest one within
/// `max_distance`. Trimmed and shortened to [`MAX_HINT_CHARS`].
pub fn ocr_hint(boxes: &[TextBox], click: Point, max_distance: f64) -> Option<String> {
    let dist = |r: &Rect| {
        let dx = (r.min_x() - click.x).max(click.x - r.max_x()).max(0.0);
        let dy = (r.min_y() - click.y).max(click.y - r.max_y()).max(0.0);
        dx.hypot(dy)
    };
    let best = boxes
        .iter()
        .filter(|b| b.text.chars().any(|c| c.is_alphanumeric()))
        .map(|b| (dist(&b.rect), b))
        .filter(|(d, _)| *d <= max_distance)
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.rect.area().total_cmp(&b.1.rect.area())))?;
    let text: String = best.1.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '|' || c == '•');
    if text.is_empty() {
        return None;
    }
    let mut out: String = text.chars().take(MAX_HINT_CHARS).collect();
    if text.chars().count() > MAX_HINT_CHARS {
        out.push('…');
    }
    Some(out)
}

fn verb(step: &Step, lang: Language) -> &'static str {
    let msg = match (step.button, step.double_click) {
        (MouseButton::Right, _) => Msg::TutorialVerbRightClick,
        (_, true) => Msg::TutorialVerbDoubleClick,
        _ => Msg::TutorialVerbClick,
    };
    msg.text(lang)
}

/// Title without AI: “Click “Save””, “Click in Untitled — Notes”,
/// “Click here” (and their Chinese forms).
pub fn local_title(step: &Step, lang: Language) -> String {
    let v = verb(step, lang);
    if let Some(hint) = step.ocr_hint.as_deref().filter(|h| !h.trim().is_empty()) {
        return format_args_into(Msg::TutorialStepOn.text(lang), &[&v, &hint]);
    }
    let place = step.window_title.as_deref().or(step.app.as_deref()).filter(|s| !s.trim().is_empty());
    match place {
        Some(p) => format_args_into(Msg::TutorialStepIn.text(lang), &[&v, &p]),
        None => format_args_into(Msg::TutorialStepHere.text(lang), &[&v]),
    }
}

/// Detail without AI: which app and window, when known.
pub fn local_detail(step: &Step, lang: Language) -> String {
    match (step.app.as_deref(), step.window_title.as_deref()) {
        (Some(app), Some(w)) if app != w => format_args_into(Msg::TutorialInAppWindow.text(lang), &[&app, &w]),
        (Some(app), _) => format_args_into(Msg::TutorialInApp.text(lang), &[&app]),
        (None, Some(w)) => format_args_into(Msg::TutorialInApp.text(lang), &[&w]),
        (None, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_log::{AppContext, Modifiers, RecordingInfo};

    fn click(t: f64, x: f64, y: f64, count: u32) -> ClickEvent {
        ClickEvent {
            t,
            button: MouseButton::Left,
            click_count: count,
            modifiers: Modifiers::default(),
            display_id: Some(1),
            display_point: [x / 2.0, y / 2.0],
            display_pixel: [x, y],
            video_pixel: Some([x, y]),
            inside_recording: true,
            app: AppContext {
                bundle_id: Some("com.apple.Notes".into()),
                name: Some("Notes".into()),
                window_title: Some("Groceries".into()),
            },
        }
    }

    fn log(clicks: Vec<ClickEvent>) -> EventLog {
        EventLog {
            schema_version: 1,
            recording: RecordingInfo {
                movie: "Rec 1.mp4".into(),
                display_id: 1,
                capture_rect: [0.0, 0.0, 1512.0, 982.0],
                scale: 2.0,
                video_width: 3024,
                video_height: 1964,
                duration: 10.0,
            },
            clicks,
        }
    }

    #[test]
    fn paths() {
        let m = Path::new("/a/Rec 1.mp4");
        assert_eq!(sidecar_path(m), PathBuf::from("/a/Rec 1.steps.json"));
        assert_eq!(image_dir(m), PathBuf::from("/a/Rec 1.tutorial"));
        assert_eq!(image_file_name(3), "step-03.png");
        assert!(is_movie(Path::new("x.MOV")) && is_movie(m) && !is_movie(Path::new("x.png")));
    }

    #[test]
    fn drops_outside_and_merges_double_clicks() {
        let mut outside = click(0.5, 10.0, 10.0, 1);
        outside.inside_recording = false;
        outside.video_pixel = None;
        let clicks = vec![
            click(3.0, 500.0, 500.0, 1),
            outside,
            click(1.0, 100.0, 100.0, 1),
            click(1.2, 104.0, 101.0, 2),  // double click on the same spot
            click(1.25, 900.0, 900.0, 1), // within min gap → collapsed
            click(2.0, 100.0, 100.0, 1),  // same place, later → new step
        ];
        let (steps, dropped) = build_steps(&log(clicks), &BuildOptions::default());
        assert_eq!(dropped, 0);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps.iter().map(|s| s.index).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(steps[0].t, 1.0);
        assert!(steps[0].double_click);
        assert_eq!(steps[0].click, [100.0, 100.0]);
        assert_eq!(steps[0].frame_time, 1.37, "frame follows the last click of the gesture");
        assert!(!steps[1].double_click);
        assert_eq!(steps[2].click, [500.0, 500.0]);
        assert_eq!(steps[2].app.as_deref(), Some("Notes"));
    }

    #[test]
    fn rapid_clicks_far_apart_and_cap() {
        let clicks: Vec<ClickEvent> =
            (0..50).map(|i| click(i as f64 * 0.19, (i * 100 % 3000) as f64, 50.0, 1)).collect();
        let opts = BuildOptions::default();
        let (steps, dropped) = build_steps(&log(clicks), &opts);
        assert_eq!(steps.len(), 40);
        assert_eq!(dropped, 10);
        assert_eq!(steps.last().unwrap().index, 40);
    }

    #[test]
    fn frame_time_clamps_to_duration() {
        assert_eq!(frame_time(1.0, 10.0, 0.12), 1.12);
        assert_eq!(frame_time(9.99, 10.0, 0.12), 9.99);
        assert_eq!(frame_time(9.5, 10.0, 0.12), 9.62);
        assert_eq!(frame_time(0.0, 0.0, 0.12), 0.12);
    }

    #[test]
    fn crop_is_clamped_and_keeps_aspect() {
        let video = Size::new(3024.0, 1964.0);
        let r = crop_rect(Point::new(10.0, 10.0), video, 1280.0);
        assert_eq!((r.min_x(), r.min_y()), (0.0, 0.0));
        assert_eq!(r.width(), 1512.0);
        assert!((r.width() / r.height() - 3024.0 / 1964.0).abs() < 0.01);
        let r = crop_rect(Point::new(3000.0, 1900.0), video, 1280.0);
        assert_eq!(r.max_x(), 3024.0);
        assert_eq!(r.max_y(), 1964.0);
        let mid = crop_rect(Point::new(1512.0, 982.0), video, 1280.0);
        assert_eq!(mid.mid(), Point::new(1512.0, 982.0));
        // Small video: whole frame.
        let small = crop_rect(Point::new(5.0, 5.0), Size::new(800.0, 600.0), 1280.0);
        assert_eq!(small, Rect::new(0.0, 0.0, 800.0, 600.0));
        assert!(crop_rect(Point::ZERO, Size::new(0.0, 0.0), 10.0).is_empty());
    }

    #[test]
    fn ocr_region_and_hint() {
        let r = ocr_rect(Point::new(20.0, 20.0), Size::new(3024.0, 1964.0), 2.0);
        assert_eq!(r, Rect::new(0.0, 0.0, 720.0, 144.0));
        let tiny = ocr_rect(Point::new(20.0, 20.0), Size::new(100.0, 50.0), 2.0);
        assert_eq!(tiny, Rect::new(0.0, 0.0, 100.0, 50.0));

        let boxes = vec![
            TextBox::new("File", Rect::new(0.0, 0.0, 40.0, 20.0)),
            TextBox::new("  Save   As… ", Rect::new(100.0, 0.0, 80.0, 20.0)),
            TextBox::new("|", Rect::new(95.0, 0.0, 4.0, 20.0)),
        ];
        assert_eq!(ocr_hint(&boxes, Point::new(120.0, 10.0), 30.0).as_deref(), Some("Save As…"));
        assert_eq!(ocr_hint(&boxes, Point::new(60.0, 10.0), 30.0).as_deref(), Some("File"));
        assert_eq!(ocr_hint(&boxes, Point::new(500.0, 300.0), 30.0), None);
        let long = vec![TextBox::new("x".repeat(80), Rect::new(0.0, 0.0, 10.0, 10.0))];
        assert_eq!(ocr_hint(&long, Point::new(5.0, 5.0), 0.0).unwrap().chars().count(), MAX_HINT_CHARS + 1);
    }

    #[test]
    fn local_titles_in_both_languages() {
        let (mut steps, _) = build_steps(&log(vec![click(1.0, 10.0, 10.0, 1)]), &BuildOptions::default());
        let s = &mut steps[0];
        assert_eq!(local_title(s, Language::En), "Click in Groceries");
        assert_eq!(local_title(s, Language::ZhHans), "在 Groceries 中点击");
        assert_eq!(local_detail(s, Language::En), "In Notes — Groceries.");
        s.ocr_hint = Some("Save".into());
        assert_eq!(local_title(s, Language::En), "Click “Save”");
        assert_eq!(local_title(s, Language::ZhHans), "点击“Save”");
        s.double_click = true;
        assert_eq!(local_title(s, Language::En), "Double-click “Save”");
        s.ocr_hint = None;
        s.window_title = None;
        s.app = None;
        s.button = MouseButton::Right;
        assert_eq!(local_title(s, Language::En), "Right-click here");
        assert_eq!(local_title(s, Language::ZhHans), "在此处右键点击");
        assert_eq!(local_detail(s, Language::En), "");
    }

    #[test]
    fn sidecar_roundtrip_and_renumber() {
        let (mut t, _) = tutorial_from_log(
            &log(vec![click(1.0, 10.0, 10.0, 1), click(3.0, 900.0, 10.0, 1), click(5.0, 10.0, 900.0, 1)]),
            &BuildOptions::default(),
            Language::En,
        );
        assert_eq!(t.title, "Rec 1");
        assert_eq!(t.scale, 2.0);
        t.steps[0].image = Some("Rec 1.tutorial/step-01.png".into());
        t.steps[0].crop = Some([0.0, 0.0, 1512.0, 982.0]);
        let json = t.to_json();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(!json.contains("double_click"), "false flags are omitted");
        assert_eq!(Tutorial::from_json(&json).unwrap(), t);
        t.remove_step(2);
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.steps[1].index, 2);
        assert_eq!(t.steps[1].t, 5.0);
        // Minimal hand-written file with defaults.
        let min = r#"{"schema_version":1,"movie":"a.mp4","video_width":10,"video_height":10,
            "steps":[{"index":1,"t":0,"frame_time":0.1,"click":[1,2],"button":"left","title":"x"}]}"#;
        let t = Tutorial::from_json(min).unwrap();
        assert_eq!(t.scale, 1.0);
        assert_eq!(t.steps[0].detail, "");
    }

    #[test]
    fn image_paths_cannot_escape_the_folder() {
        assert!(is_safe_relative("Rec 1.tutorial/step-01.png"));
        for bad in ["", "/etc/passwd", "../secret.png", "Rec.tutorial/../../x.png", "./x.png", "a\\..\\b.png"] {
            assert!(!is_safe_relative(bad), "{bad:?}");
        }
        let json = r#"{"schema_version":1,"movie":"a.mp4","video_width":10,"video_height":10,"steps":[
            {"index":1,"t":0,"frame_time":0,"click":[1,2],"button":"left","title":"x","image":"a.tutorial/step-01.png"},
            {"index":2,"t":0,"frame_time":0,"click":[1,2],"button":"left","title":"y","image":"../../.ssh/id_rsa"},
            {"index":3,"t":0,"frame_time":0,"click":[1,2],"button":"left","title":"z","image":"/Users/a/x.png"}]}"#;
        let t = Tutorial::from_json(json).unwrap();
        let imgs: Vec<_> = t.steps.iter().map(|s| s.image.as_deref()).collect();
        assert_eq!(imgs, vec![Some("a.tutorial/step-01.png"), None, None]);
    }

    #[test]
    fn relocate_follows_a_renamed_movie() {
        let (mut t, _) = tutorial_from_log(
            &log(vec![click(1.0, 10.0, 10.0, 1), click(3.0, 900.0, 10.0, 1)]),
            &BuildOptions::default(),
            Language::En,
        );
        t.steps[0].image = Some("Rec 1.tutorial/step-01.png".into());
        t.steps[1].image = Some("elsewhere/step-02.png".into());
        // Same name, new folder: only `movie` may change.
        assert!(
            !t.relocate(Path::new("/tmp/Lumora/Rec 1.mov"), Path::new("/Users/a/Desktop/Rec 1.mov"))
                || t.movie == "Rec 1.mov"
        );
        assert_eq!(t.steps[0].image.as_deref(), Some("Rec 1.tutorial/step-01.png"));
        // Renamed on Save (unique name).
        assert!(t.relocate(Path::new("/tmp/Lumora/Rec 1.mov"), Path::new("/Users/a/Desktop/Rec 1 2.mov")));
        assert_eq!(t.movie, "Rec 1 2.mov");
        assert_eq!(t.steps[0].image.as_deref(), Some("Rec 1 2.tutorial/step-01.png"));
        assert_eq!(t.steps[1].image.as_deref(), Some("elsewhere/step-02.png"));
    }
}
