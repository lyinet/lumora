//! Burned-in subtitles: the pure layout.
//!
//! The capture crate draws one Core Animation text layer per [`Cue`] over
//! the movie (AVVideoCompositionCoreAnimationTool). Everything it needs is
//! decided here: the font size from the video size and [`SubtitleSize`],
//! the line budget, CJK-aware wrapping into at most [`MAX_LINES`] lines
//! (longer segments become timed sub-cues), the background box and where it
//! sits — in Core Animation's bottom-left-origin coordinates — and the
//! opacity keyframes that show each cue only during its time.

use serde::{Deserialize, Serialize};

use super::{join_text, split_proportional, wrap_lines, Segment, MAX_CJK, MAX_LATIN, MAX_LINES};
use crate::geometry::Rect;

/// Subtitle text size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl SubtitleSize {
    pub const ALL: [SubtitleSize; 3] = [SubtitleSize::Small, SubtitleSize::Medium, SubtitleSize::Large];

    /// Font size as a fraction of the video's shorter side.
    pub fn factor(self) -> f64 {
        match self {
            SubtitleSize::Small => 0.040,
            SubtitleSize::Medium => 0.050,
            SubtitleSize::Large => 0.062,
        }
    }
}

/// Where subtitles sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitlePosition {
    #[default]
    Bottom,
    Top,
}

/// The style chosen in the transcript window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubtitleStyle {
    pub size: SubtitleSize,
    pub position: SubtitlePosition,
    /// Draw a semi-transparent black rounded box behind the text.
    pub background: bool,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        SubtitleStyle { size: SubtitleSize::Medium, position: SubtitlePosition::Bottom, background: true }
    }
}

/// Smallest font size (pixels) we draw at.
pub const MIN_FONT: f64 = 12.0;
/// Text may span this fraction of the video width.
pub const MAX_WIDTH_FRACTION: f64 = 0.86;
/// Gap between the box and the top/bottom edge, as a fraction of the height.
pub const MARGIN_FRACTION: f64 = 0.06;
/// Estimated advance of a non-CJK character, in ems (bold system font).
pub const LATIN_EM: f64 = 0.56;
/// Line height, in ems (set explicitly on the text so CJK fallback fonts
/// don't change it).
pub const LINE_HEIGHT_EM: f64 = 1.3;
/// Background box opacity.
pub const BOX_ALPHA: f64 = 0.6;

/// Sizes derived from the video and the style.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurnLayout {
    pub video_w: f64,
    pub video_h: f64,
    pub font_size: f64,
    pub line_height: f64,
    /// Widest the text may be (pixels).
    pub max_text_width: f64,
    /// Line budget handed to [`wrap_lines`].
    pub max_latin: usize,
    pub max_cjk: usize,
    pub pad_x: f64,
    pub pad_y: f64,
    pub corner_radius: f64,
    /// Gap between the box and the video edge.
    pub margin: f64,
    pub style: SubtitleStyle,
}

impl BurnLayout {
    /// Layout for a `video_w`×`video_h` pixel video.
    pub fn new(video_w: f64, video_h: f64, style: SubtitleStyle) -> BurnLayout {
        let video_w = sane(video_w).max(16.0);
        let video_h = sane(video_h).max(16.0);
        let font_size = (video_w.min(video_h) * style.size.factor()).max(MIN_FONT).round();
        let pad_x = (font_size * 0.5).round();
        let pad_y = (font_size * 0.25).round();
        let max_text_width = (video_w * MAX_WIDTH_FRACTION - 2.0 * pad_x).max(font_size);
        // Fit the budget to the width, but never beyond the SRT budget:
        // long lines are hard to read however wide the video is.
        let max_cjk = ((max_text_width / font_size).floor() as usize).clamp(1, MAX_CJK);
        let max_latin = ((max_text_width / (font_size * LATIN_EM)).floor() as usize).clamp(2, MAX_LATIN);
        BurnLayout {
            video_w,
            video_h,
            font_size,
            line_height: (font_size * LINE_HEIGHT_EM).round(),
            max_text_width,
            max_latin,
            max_cjk,
            pad_x,
            pad_y,
            corner_radius: (font_size * 0.3).round(),
            margin: (video_h * MARGIN_FRACTION).round(),
            style,
        }
    }

    /// Estimated rendered width of `line` (pixels).
    pub fn text_width(&self, line: &str) -> f64 {
        line.chars().map(|c| if super::is_cjk(c) { 1.0 } else { LATIN_EM }).sum::<f64>() * self.font_size
    }

    /// Wrap `text` to this layout's budget.
    pub fn wrap(&self, text: &str) -> Vec<String> {
        wrap_lines(text, self.max_latin, self.max_cjk)
    }

    /// Box and text frames for `lines`, in Core Animation coordinates
    /// (origin bottom-left, y up).
    pub fn frames(&self, lines: &[String]) -> (Rect, Rect) {
        let n = lines.len().max(1) as f64;
        let text_h = n * self.line_height;
        let widest = lines.iter().map(|l| self.text_width(l)).fold(0.0, f64::max).min(self.max_text_width);
        let box_w = (widest + 2.0 * self.pad_x).min(self.video_w);
        let box_h = text_h + 2.0 * self.pad_y;
        let top = match self.style.position {
            SubtitlePosition::Bottom => self.video_h - self.margin - box_h,
            SubtitlePosition::Top => self.margin,
        };
        let boxed = flip_y(Rect::new((self.video_w - box_w) / 2.0, top, box_w, box_h), self.video_h);
        // The text layer is as wide as the text may get, centred: an
        // under-estimated width then never wraps or clips the text.
        let text = flip_y(
            Rect::new((self.video_w - self.max_text_width) / 2.0, top + self.pad_y, self.max_text_width, text_h),
            self.video_h,
        );
        (boxed, text)
    }
}

/// A top-left-origin rect in a `height`-tall space, in bottom-left-origin
/// (Core Animation) coordinates.
pub fn flip_y(r: Rect, height: f64) -> Rect {
    Rect::new(r.min_x(), height - r.min_y() - r.height(), r.width(), r.height())
}

/// One subtitle on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    /// At most [`MAX_LINES`] lines.
    pub lines: Vec<String>,
    /// Background box (Core Animation coordinates).
    pub box_rect: Rect,
    /// Text layer frame (Core Animation coordinates).
    pub text_rect: Rect,
}

impl Cue {
    /// The lines joined for the text layer.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// The cues for `segments` in a movie of `duration` seconds. Empty
/// segments are skipped, times are clamped to the movie, and a segment that
/// wraps to more than [`MAX_LINES`] lines is split into timed sub-cues,
/// dividing its time by character count.
pub fn layout_cues(segments: &[Segment], layout: &BurnLayout, duration: f64) -> Vec<Cue> {
    let duration = sane(duration).max(0.0);
    let mut out = Vec::new();
    for seg in segments {
        let text = seg.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let (start, end) = (sane(seg.start).clamp(0.0, duration), sane(seg.end).clamp(0.0, duration));
        if text.is_empty() || end <= start {
            continue;
        }
        let lines = layout.wrap(&text);
        let groups: Vec<Vec<String>> = lines.chunks(MAX_LINES).map(<[String]>::to_vec).collect();
        let pieces: Vec<String> =
            groups.iter().map(|g| g.iter().fold(String::new(), |acc, l| join_text(&acc, l))).collect();
        for (piece, lines) in split_proportional(start, end, pieces).into_iter().zip(groups) {
            let (box_rect, text_rect) = layout.frames(&lines);
            out.push(Cue { start: piece.start, end: piece.end, lines, box_rect, text_rect });
        }
    }
    out
}

/// Opacity keyframes that show a cue during `[start, end)` of a
/// `duration`-second animation, for a discrete `CAKeyframeAnimation`:
/// `(values, key_times)` with one more key time than values (`[0, s, e, 1]`
/// for values `[0, 1, 0]`). `None` when the cue is not visible at all.
pub fn opacity_keyframes(start: f64, end: f64, duration: f64) -> Option<(Vec<f64>, Vec<f64>)> {
    let d = sane(duration);
    if d <= 0.0 {
        return None;
    }
    let s = (sane(start) / d).clamp(0.0, 1.0);
    let e = (sane(end) / d).clamp(0.0, 1.0);
    if e <= s {
        return None;
    }
    Some((vec![0.0, 1.0, 0.0], vec![0.0, s, e, 1.0]))
}

fn sane(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(size: SubtitleSize, position: SubtitlePosition) -> SubtitleStyle {
        SubtitleStyle { size, position, background: true }
    }

    #[test]
    fn font_size_scales_with_short_side_and_option() {
        let s = |size| BurnLayout::new(1920.0, 1080.0, style(size, SubtitlePosition::Bottom)).font_size;
        assert_eq!(s(SubtitleSize::Medium), 54.0);
        assert!(s(SubtitleSize::Small) < s(SubtitleSize::Medium));
        assert!(s(SubtitleSize::Medium) < s(SubtitleSize::Large));
        // Portrait uses the width, so text still fits.
        let portrait = BurnLayout::new(1080.0, 1920.0, SubtitleStyle::default());
        assert_eq!(portrait.font_size, 54.0);
        // Tiny videos keep a readable minimum.
        assert_eq!(BurnLayout::new(100.0, 80.0, SubtitleStyle::default()).font_size, MIN_FONT);
        // Garbage sizes don't panic.
        let g = BurnLayout::new(f64::NAN, -5.0, SubtitleStyle::default());
        assert!(g.font_size >= MIN_FONT && g.max_cjk >= 1);
    }

    #[test]
    fn line_budget_fits_width_and_is_capped() {
        let wide = BurnLayout::new(3840.0, 1080.0, SubtitleStyle::default());
        assert_eq!((wide.max_latin, wide.max_cjk), (MAX_LATIN, MAX_CJK));
        let narrow = BurnLayout::new(720.0, 1280.0, SubtitleStyle::default());
        assert!(narrow.max_cjk < MAX_CJK && narrow.max_latin < MAX_LATIN);
        // The budget really fits: a full CJK line is within the text width.
        let line: String = "字".repeat(narrow.max_cjk);
        assert!(narrow.text_width(&line) <= narrow.max_text_width);
        assert!(narrow.max_text_width <= 720.0 * MAX_WIDTH_FRACTION);
    }

    #[test]
    fn frames_bottom_and_top_in_flipped_coordinates() {
        let l = BurnLayout::new(1920.0, 1080.0, style(SubtitleSize::Medium, SubtitlePosition::Bottom));
        let lines = vec!["Hello world".to_string()];
        let (b, t) = l.frames(&lines);
        // Bottom: the box sits `margin` above the bottom edge (y up).
        assert_eq!(b.min_y(), l.margin);
        assert_eq!(b.height(), l.line_height + 2.0 * l.pad_y);
        assert!((b.mid().x - 960.0).abs() < 1e-9);
        // Text is inside the box vertically and centred horizontally.
        assert_eq!(t.min_y(), b.min_y() + l.pad_y);
        assert_eq!(t.height(), l.line_height);
        assert!((t.mid().x - 960.0).abs() < 1e-9);

        let top = BurnLayout::new(1920.0, 1080.0, style(SubtitleSize::Medium, SubtitlePosition::Top));
        let (b2, _) = top.frames(&[String::from("a"), String::from("b")]);
        assert_eq!(b2.max_y(), 1080.0 - top.margin);
        assert_eq!(b2.height(), 2.0 * top.line_height + 2.0 * top.pad_y);
    }

    #[test]
    fn box_width_follows_text_and_is_capped() {
        let l = BurnLayout::new(1920.0, 1080.0, SubtitleStyle::default());
        let (short, _) = l.frames(&["Hi".into()]);
        let (long, _) = l.frames(&["x".repeat(500)]);
        assert!(short.width() < long.width());
        assert!(long.width() <= l.max_text_width + 2.0 * l.pad_x + 1e-9);
        let (cjk, _) = l.frames(&["你好世界".into()]);
        assert_eq!(cjk.width(), 4.0 * l.font_size + 2.0 * l.pad_x);
    }

    #[test]
    fn flip_y_round_trips() {
        let r = Rect::new(10.0, 20.0, 30.0, 40.0);
        let f = flip_y(r, 100.0);
        assert_eq!(f, Rect::new(10.0, 40.0, 30.0, 40.0));
        assert_eq!(flip_y(f, 100.0), r);
    }

    #[test]
    fn cues_skip_empty_clamp_and_split_long_segments() {
        let l = BurnLayout::new(1280.0, 720.0, SubtitleStyle::default());
        let long_zh: String = "字".repeat(l.max_cjk * 5);
        let segs = vec![
            Segment::new(0.0, 2.0, "  hello   there "),
            Segment::new(2.0, 3.0, "   "),
            Segment::new(3.0, 13.0, long_zh),
            Segment::new(14.0, 30.0, "past the end"),
            Segment::new(25.0, 26.0, "after the movie"),
        ];
        let cues = layout_cues(&segs, &l, 20.0);
        assert_eq!(cues[0].lines, vec!["hello there".to_string()]);
        // 5 lines → 3 sub-cues (2 + 2 + 1 lines), time split by characters.
        let sub: Vec<&Cue> = cues.iter().filter(|c| c.start >= 3.0 && c.end <= 13.0).collect();
        assert_eq!(sub.len(), 3);
        assert!(sub.iter().all(|c| c.lines.len() <= MAX_LINES));
        assert_eq!(sub[0].start, 3.0);
        assert!((sub[0].end - 7.0).abs() < 1e-9);
        assert!((sub[1].end - 11.0).abs() < 1e-9);
        assert_eq!(sub[2].end, 13.0);
        assert!(sub[2].box_rect.height() < sub[0].box_rect.height());
        // Clamped to the movie; the one after it is dropped.
        let last = cues.last().unwrap();
        assert_eq!((last.start, last.end), (14.0, 20.0));
        assert_eq!(cues.len(), 5);
        assert_eq!(sub[0].text().matches('\n').count(), 1);
    }

    #[test]
    fn keyframes_show_only_during_the_cue() {
        let (v, k) = opacity_keyframes(2.0, 4.0, 10.0).unwrap();
        assert_eq!(v, vec![0.0, 1.0, 0.0]);
        assert_eq!(k, vec![0.0, 0.2, 0.4, 1.0]);
        assert_eq!(k.len(), v.len() + 1);
        assert!(opacity_keyframes(4.0, 4.0, 10.0).is_none());
        assert!(opacity_keyframes(1.0, 2.0, 0.0).is_none());
        let (_, k) = opacity_keyframes(-1.0, 99.0, 10.0).unwrap();
        assert_eq!(k, vec![0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn style_serde_names() {
        assert_eq!(serde_json::to_string(&SubtitleSize::Large).unwrap(), "\"large\"");
        assert_eq!(serde_json::to_string(&SubtitlePosition::Top).unwrap(), "\"top\"");
    }
}
