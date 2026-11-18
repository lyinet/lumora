//! Trimming recordings and turning them into GIFs: the pure parts.
//!
//! The app picks a [`TrimRange`] in the trim window, exports the movie with
//! AVFoundation (passthrough) and rewrites the sidecars with
//! [`trim_event_log`] / [`trim_transcript`] so click times and subtitles
//! still line up with the shorter movie. A GIF uses the same range; its
//! frame times, per-frame delays, output size and a size estimate come from
//! [`GifPlan`].

use std::path::{Path, PathBuf};

use crate::click_log::EventLog;
use crate::transcript::chapters::Chapter;
use crate::transcript::{Segment, Transcript};

/// Shortest clip the trim window allows (seconds).
pub const MIN_CLIP: f64 = 0.5;

/// Longest GIF, in frames (memory: ImageIO keeps every frame until the
/// file is finalised).
pub const MAX_GIF_FRAMES: usize = 1200;

/// A `[start, end]` range within a movie, in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrimRange {
    pub start: f64,
    pub end: f64,
}

impl TrimRange {
    /// The whole movie.
    pub fn full(duration: f64) -> TrimRange {
        TrimRange { start: 0.0, end: finite(duration).max(0.0) }
    }

    /// Keep the range inside `[0, duration]`, ordered, and at least
    /// [`MIN_CLIP`] long (or the whole movie when it is shorter). Moving
    /// `start` past `end` pushes `end` along, and vice versa.
    pub fn clamped(self, duration: f64) -> TrimRange {
        let d = finite(duration).max(0.0);
        let min = MIN_CLIP.min(d);
        let mut start = finite(self.start).clamp(0.0, d);
        let mut end = finite(self.end).clamp(0.0, d);
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        if end - start < min {
            if start + min <= d {
                end = start + min;
            } else {
                end = d;
                start = d - min;
            }
        }
        TrimRange { start, end }
    }

    pub fn len(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() <= 0.0
    }

    /// Whether the range covers (practically) the whole movie.
    pub fn is_full(&self, duration: f64) -> bool {
        self.start <= 0.01 && self.end >= finite(duration) - 0.01
    }
}

fn finite(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Trim-window time label: `0:07.4`, `1:02:03.0`.
pub fn time_label(seconds: f64) -> String {
    let tenths = (finite(seconds).max(0.0) * 10.0).round() as u64;
    let (t, s) = (tenths % 10, tenths / 10);
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{t}")
    } else {
        format!("{m}:{s:02}.{t}")
    }
}

/// `<stem> (trimmed).<ext>` next to `movie`, numbered when taken.
pub fn trimmed_path(movie: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    sibling(movie, "trimmed", None, exists)
}

/// `<stem>.gif` next to `movie` (`<stem> (trimmed).gif` for a part of it),
/// numbered when taken.
pub fn gif_path(movie: &Path, trimmed: bool, exists: impl Fn(&Path) -> bool) -> PathBuf {
    sibling(movie, if trimmed { "trimmed" } else { "" }, Some("gif"), exists)
}

fn sibling(movie: &Path, tag: &str, ext: Option<&str>, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let dir = movie.parent().unwrap_or_else(|| Path::new("."));
    let stem = movie.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Recording".into());
    let ext = ext
        .map(str::to_string)
        .or_else(|| movie.extension().map(|e| e.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "mov".into());
    let stem = if tag.is_empty() { stem } else { format!("{stem} ({tag})") };
    crate::filename::unique_path(dir, &stem, &ext, exists)
}

// ---------------------------------------------------------------- sidecars --

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The click log of the trimmed movie: clicks inside `range`, shifted so
/// `range.start` is 0; the duration and movie name are updated.
pub fn trim_event_log(log: &EventLog, range: TrimRange, movie_name: &str) -> EventLog {
    let mut out = log.clone();
    out.recording.movie = movie_name.to_string();
    out.recording.duration = round3(range.len());
    out.clicks = log
        .clicks
        .iter()
        .filter(|c| c.t >= range.start && c.t <= range.end)
        .map(|c| {
            let mut c = c.clone();
            c.t = round3((c.t - range.start).max(0.0));
            c
        })
        .collect();
    out
}

/// The transcript of the trimmed movie: segments overlapping `range`,
/// clipped to it and shifted; chapters shifted, the one running at
/// `range.start` moved to 0. Returns `None` when no speech is left.
pub fn trim_transcript(t: &Transcript, range: TrimRange) -> Option<Transcript> {
    let len = range.len();
    let segments: Vec<Segment> = t
        .segments
        .iter()
        .filter(|s| s.end > range.start && s.start < range.end)
        .map(|s| {
            let start = round3((s.start - range.start).clamp(0.0, len));
            let end = round3((s.end - range.start).clamp(0.0, len));
            Segment::new(start, end.max(start), s.text.clone())
        })
        .filter(|s| s.end > s.start)
        .collect();
    if segments.is_empty() {
        return None;
    }
    let mut out = t.clone();
    out.segments = segments;
    if let Some(summary) = &mut out.summary {
        let mut chapters: Vec<Chapter> = Vec::new();
        // The chapter already running when the clip starts opens it at 0.
        if let Some(running) = summary.chapters.iter().rfind(|c| c.start <= range.start) {
            chapters.push(Chapter::new(0.0, running.title.clone()));
        }
        for c in summary.chapters.iter().filter(|c| c.start > range.start && c.start < range.end) {
            chapters.push(Chapter::new(round3(c.start - range.start), c.title.clone()));
        }
        summary.chapters = chapters;
    }
    Some(out)
}

// --------------------------------------------------------------------- GIF --

/// GIF width choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GifWidth {
    W480,
    W640,
    W800,
    Original,
}

impl GifWidth {
    pub const ALL: [GifWidth; 4] = [GifWidth::W480, GifWidth::W640, GifWidth::W800, GifWidth::Original];

    pub fn pixels(self) -> Option<u32> {
        match self {
            GifWidth::W480 => Some(480),
            GifWidth::W640 => Some(640),
            GifWidth::W800 => Some(800),
            GifWidth::Original => None,
        }
    }
}

/// GIF frame-rate choices.
pub const GIF_FPS: [u32; 3] = [10, 15, 20];

/// GIF output size for a `src_w × src_h` movie: `width` wide (never
/// upscaled), aspect kept, at least 1 × 1.
pub fn gif_size(src_w: u32, src_h: u32, width: GifWidth) -> (u32, u32) {
    let (sw, sh) = (src_w.max(1), src_h.max(1));
    let w = width.pixels().map_or(sw, |p| p.min(sw)).max(1);
    let h = ((sh as f64) * (w as f64) / (sw as f64)).round().max(1.0) as u32;
    (w, h)
}

/// Everything needed to write one GIF.
#[derive(Debug, Clone, PartialEq)]
pub struct GifPlan {
    /// Source time of each frame (seconds, movie timeline).
    pub times: Vec<f64>,
    /// Display time of each frame in hundredths of a second (GIF's unit);
    /// they add up to the clip length without drifting.
    pub delays_cs: Vec<u32>,
    pub width: u32,
    pub height: u32,
    /// The clip was longer than [`MAX_GIF_FRAMES`] allows and was cut short.
    pub capped: bool,
}

impl GifPlan {
    pub fn new(range: TrimRange, fps: u32, size: (u32, u32)) -> GifPlan {
        let fps = fps.clamp(1, 50) as f64;
        let wanted = (range.len() * fps).round().max(1.0) as usize;
        let n = wanted.min(MAX_GIF_FRAMES);
        let times = (0..n).map(|i| round3(range.start + i as f64 / fps)).collect();
        // Delay i spans [round(i·100/fps), round((i+1)·100/fps)) so the
        // centisecond rounding never accumulates; GIF readers treat a
        // delay under 2 cs as "slow", so keep at least 2.
        let cs = |i: usize| (i as f64 * 100.0 / fps).round() as u32;
        let delays_cs = (0..n).map(|i| (cs(i + 1) - cs(i)).max(2)).collect();
        GifPlan { times, delays_cs, width: size.0, height: size.1, capped: wanted > n }
    }

    pub fn frames(&self) -> usize {
        self.times.len()
    }

    /// Rough file size. ImageIO writes every frame whole (no inter-frame
    /// optimisation) with LZW over an 8-bit palette; screen content
    /// compresses to roughly a third of a byte per pixel, plus a local
    /// colour table per frame.
    pub fn estimated_bytes(&self) -> u64 {
        const BYTES_PER_PIXEL: f64 = 0.35;
        const PER_FRAME: f64 = 800.0;
        let px = self.width as f64 * self.height as f64;
        (self.frames() as f64 * (px * BYTES_PER_PIXEL + PER_FRAME) + 1024.0) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click_log::{AppContext, ClickEvent, Modifiers, MouseButton, RecordingInfo};
    use crate::transcript::chapters::Summary;

    #[test]
    fn range_clamps_orders_and_keeps_a_minimum() {
        let d = 10.0;
        assert_eq!(TrimRange { start: -1.0, end: 20.0 }.clamped(d), TrimRange { start: 0.0, end: 10.0 });
        assert_eq!(TrimRange { start: 6.0, end: 4.0 }.clamped(d), TrimRange { start: 4.0, end: 6.0 });
        assert_eq!(TrimRange { start: 3.0, end: 3.1 }.clamped(d), TrimRange { start: 3.0, end: 3.5 });
        assert_eq!(TrimRange { start: 9.9, end: 10.0 }.clamped(d), TrimRange { start: 9.5, end: 10.0 });
        // Shorter than the minimum: the whole movie.
        assert_eq!(TrimRange { start: 0.1, end: 0.2 }.clamped(0.3), TrimRange { start: 0.0, end: 0.3 });
        assert_eq!(TrimRange { start: f64::NAN, end: 5.0 }.clamped(d), TrimRange { start: 0.0, end: 5.0 });
        assert!(TrimRange::full(d).is_full(d));
        assert!(!TrimRange { start: 1.0, end: 10.0 }.is_full(d));
        assert!((TrimRange { start: 2.0, end: 5.5 }.len() - 3.5).abs() < 1e-9);
    }

    #[test]
    fn time_labels() {
        assert_eq!(time_label(0.0), "0:00.0");
        assert_eq!(time_label(7.44), "0:07.4");
        assert_eq!(time_label(62.96), "1:03.0");
        assert_eq!(time_label(3723.0), "1:02:03.0");
        assert_eq!(time_label(-3.0), "0:00.0");
    }

    #[test]
    fn output_names() {
        let movie = Path::new("/caps/Rec 1.mp4");
        assert_eq!(trimmed_path(movie, |_| false), PathBuf::from("/caps/Rec 1 (trimmed).mp4"));
        let taken = PathBuf::from("/caps/Rec 1 (trimmed).mp4");
        assert_eq!(trimmed_path(movie, |p| p == taken), PathBuf::from("/caps/Rec 1 (trimmed) (2).mp4"));
        assert_eq!(gif_path(movie, false, |_| false), PathBuf::from("/caps/Rec 1.gif"));
        assert_eq!(gif_path(movie, true, |_| false), PathBuf::from("/caps/Rec 1 (trimmed).gif"));
    }

    fn click(t: f64) -> ClickEvent {
        ClickEvent {
            t,
            button: MouseButton::Left,
            click_count: 1,
            modifiers: Modifiers::default(),
            display_id: Some(1),
            display_point: [0.0, 0.0],
            display_pixel: [0.0, 0.0],
            video_pixel: Some([0.0, 0.0]),
            inside_recording: true,
            app: AppContext::default(),
        }
    }

    #[test]
    fn event_log_is_cut_and_shifted() {
        let log = EventLog {
            schema_version: 1,
            recording: RecordingInfo {
                movie: "a.mp4".into(),
                display_id: 1,
                capture_rect: [0.0, 0.0, 100.0, 100.0],
                scale: 2.0,
                video_width: 200,
                video_height: 200,
                duration: 10.0,
            },
            clicks: vec![click(1.0), click(2.5), click(4.0), click(9.0)],
        };
        let out = trim_event_log(&log, TrimRange { start: 2.0, end: 4.0 }, "a (trimmed).mp4");
        assert_eq!(out.recording.movie, "a (trimmed).mp4");
        assert_eq!(out.recording.duration, 2.0);
        assert_eq!(out.clicks.iter().map(|c| c.t).collect::<Vec<_>>(), vec![0.5, 2.0]);
    }

    #[test]
    fn transcript_is_clipped_shifted_and_chapters_follow() {
        let mut t = Transcript::new(
            "en",
            "test",
            vec![Segment::new(0.0, 2.0, "a"), Segment::new(2.0, 5.0, "b"), Segment::new(6.0, 8.0, "c")],
        );
        t.summary = Some(Summary {
            summary: "s".into(),
            chapters: vec![Chapter::new(0.0, "Intro"), Chapter::new(4.0, "Main"), Chapter::new(7.5, "End")],
        });
        let out = trim_transcript(&t, TrimRange { start: 3.0, end: 7.0 }).unwrap();
        assert_eq!(out.segments, vec![Segment::new(0.0, 2.0, "b"), Segment::new(3.0, 4.0, "c")]);
        let ch = &out.summary.as_ref().unwrap().chapters;
        assert_eq!(ch, &vec![Chapter::new(0.0, "Intro"), Chapter::new(1.0, "Main")]);
        assert_eq!(out.summary.as_ref().unwrap().summary, "s");
        // Nothing said in the clip.
        assert!(trim_transcript(&t, TrimRange { start: 5.1, end: 5.9 }).is_none());
    }

    #[test]
    fn gif_size_scales_down_only() {
        assert_eq!(gif_size(1920, 1080, GifWidth::W640), (640, 360));
        assert_eq!(gif_size(1920, 1080, GifWidth::Original), (1920, 1080));
        assert_eq!(gif_size(400, 300, GifWidth::W800), (400, 300));
        assert_eq!(gif_size(0, 0, GifWidth::W480), (1, 1));
    }

    #[test]
    fn gif_frame_schedule_and_delays() {
        let p = GifPlan::new(TrimRange { start: 1.0, end: 3.0 }, 15, (640, 360));
        assert_eq!(p.frames(), 30);
        assert_eq!(p.times[0], 1.0);
        assert!((p.times[29] - (1.0 + 29.0 / 15.0)).abs() < 1e-3);
        assert!(p.times.windows(2).all(|w| w[1] > w[0]));
        // 15 fps → 7, 6, 7, 6, 7 … adding up to the clip length.
        assert_eq!(p.delays_cs.iter().sum::<u32>(), 200);
        assert!(p.delays_cs.iter().all(|&d| d == 6 || d == 7));
        let p = GifPlan::new(TrimRange { start: 0.0, end: 1.0 }, 20, (10, 10));
        assert_eq!(p.delays_cs, vec![5; 20]);
        assert!(!p.capped);
        // A tiny clip still gets one frame.
        assert_eq!(GifPlan::new(TrimRange { start: 0.0, end: 0.01 }, 10, (1, 1)).frames(), 1);
    }

    #[test]
    fn gif_is_capped_and_estimated() {
        let long = GifPlan::new(TrimRange { start: 0.0, end: 600.0 }, 20, (640, 360));
        assert!(long.capped);
        assert_eq!(long.frames(), MAX_GIF_FRAMES);
        let small = GifPlan::new(TrimRange { start: 0.0, end: 5.0 }, 10, (480, 270));
        let big = GifPlan::new(TrimRange { start: 0.0, end: 5.0 }, 20, (800, 450));
        assert!(small.estimated_bytes() > 100_000 && small.estimated_bytes() < 5_000_000);
        assert!(big.estimated_bytes() > 4 * small.estimated_bytes());
    }
}
