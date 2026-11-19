//! Live preview of burned-in subtitles in the transcript window.
//!
//! The preview must look exactly like the export, so it never lays text out
//! on its own: [`cues_at`] runs the export's own [`layout_cues`] (on just the
//! segments that can be on screen) and keeps the cues the export's opacity
//! keyframes would show at that time. The window then draws those cues with
//! the export's layer code at the video's pixel size and scales the whole
//! tree down to the pane ([`fit`]), the same way a player scales the movie.
//!
//! Also here: the scrubber's time maths and a small frame cache keyed by
//! time bucket, so scrubbing back and forth doesn't regenerate frames.

use super::burn::{layout_cues, BurnLayout, Cue};
use super::Segment;
use crate::geometry::Rect;

/// Frames are cached and requested per bucket of this many seconds (the
/// image generator's tolerance is about the same, so a finer key would only
/// fetch the same frame twice).
pub const FRAME_BUCKET: f64 = 0.05;
/// Frames kept by the preview's [`FrameCache`].
pub const FRAME_CACHE_CAP: usize = 16;
/// Debounce before a style change, edit or scrub asks for a new frame.
pub const DEBOUNCE_SECS: f64 = 0.1;

/// Whether the export shows `cue` at `t` — its discrete opacity animation
/// is 1 on `[start, end)`.
pub fn visible_at(cue: &Cue, t: f64) -> bool {
    cue.start <= t && t < cue.end
}

/// The cues the export shows at `t` (seconds) in a `duration`-second movie,
/// laid out by [`layout_cues`] — identical, field for field, to the export's
/// cues at that time. Usually zero or one; overlapping segments give
/// several, drawn in order like the export's stacked layers.
pub fn cues_at(segments: &[Segment], layout: &BurnLayout, duration: f64, t: f64) -> Vec<Cue> {
    if !t.is_finite() {
        return Vec::new();
    }
    // Each segment is laid out independently and its cues stay inside its
    // (clamped) time, so skipping segments that can't cover `t` changes
    // nothing — it only avoids laying out a whole long transcript per frame.
    let d = if duration.is_finite() { duration.max(0.0) } else { 0.0 };
    let clamp = |v: f64| if v.is_finite() { v.clamp(0.0, d) } else { 0.0 };
    let near: Vec<Segment> = segments.iter().filter(|s| clamp(s.start) <= t && t < clamp(s.end)).cloned().collect();
    layout_cues(&near, layout, duration).into_iter().filter(|c| visible_at(c, t)).collect()
}

/// How a `video_w`×`video_h` frame sits in a `view_w`×`view_h` pane:
/// scaled to fit (up as well as down, so small recordings fill the pane
/// too) and centred.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// View points per video pixel.
    pub scale: f64,
    /// Where the video lands in the view (same origin convention as the
    /// view; it is centred, so top-left and bottom-left agree).
    pub frame: Rect,
}

/// Aspect-fit a video into a pane. Degenerate sizes give a zero scale.
pub fn fit(video_w: f64, video_h: f64, view_w: f64, view_h: f64) -> Fit {
    let ok = |v: f64| v.is_finite() && v > 0.0;
    if !(ok(video_w) && ok(video_h) && ok(view_w) && ok(view_h)) {
        return Fit { scale: 0.0, frame: Rect::new(0.0, 0.0, 0.0, 0.0) };
    }
    let scale = (view_w / video_w).min(view_h / video_h);
    let (w, h) = (video_w * scale, video_h * scale);
    Fit { scale, frame: Rect::new((view_w - w) / 2.0, (view_h - h) / 2.0, w, h) }
}

/// The text layers' `contentsScale` in the preview: the export renders at
/// 1 px per video pixel; the preview shows a video pixel at `scale` points,
/// i.e. `scale × backing` screen pixels, so rasterise at that to stay crisp.
pub fn contents_scale(scale: f64, backing: f64) -> f64 {
    let v = scale * backing;
    if v.is_finite() && v > 0.0 {
        v.clamp(0.05, 4.0)
    } else {
        1.0
    }
}

/// The cache key (time bucket) for `t`.
pub fn frame_key(t: f64) -> i64 {
    if t.is_finite() {
        (t.max(0.0) / FRAME_BUCKET).round() as i64
    } else {
        0
    }
}

/// The time a bucket's frame is requested at.
pub fn key_time(key: i64) -> f64 {
    key.max(0) as f64 * FRAME_BUCKET
}

/// Where a segment's preview frame comes from: its midpoint.
pub fn midpoint(seg: &Segment) -> f64 {
    let (s, e) = (seg.start, seg.end);
    if s.is_finite() && e.is_finite() {
        (s + e.max(s)) / 2.0
    } else {
        0.0
    }
}

/// Clamp a scrub position to the movie; a frame can't be fetched at the
/// very end, so stop just before it.
pub fn clamp_time(t: f64, duration: f64) -> f64 {
    let d = if duration.is_finite() { duration.max(0.0) } else { 0.0 };
    let last = (d - FRAME_BUCKET / 2.0).max(0.0);
    if t.is_finite() {
        t.clamp(0.0, last)
    } else {
        0.0
    }
}

/// The index of the segment the export shows at `t` (first of several),
/// for highlighting it in the list.
pub fn segment_at(segments: &[Segment], t: f64) -> Option<usize> {
    segments.iter().position(|s| !s.text.trim().is_empty() && s.start <= t && t < s.end)
}

/// A small least-recently-used cache of preview frames by [`frame_key`].
#[derive(Debug)]
pub struct FrameCache<T> {
    cap: usize,
    /// Oldest first.
    items: Vec<(i64, T)>,
}

impl<T: Clone> FrameCache<T> {
    pub fn new(cap: usize) -> Self {
        FrameCache { cap: cap.max(1), items: Vec::new() }
    }

    /// The frame for `key`, marking it most recently used.
    pub fn get(&mut self, key: i64) -> Option<T> {
        let i = self.items.iter().position(|(k, _)| *k == key)?;
        let item = self.items.remove(i);
        let v = item.1.clone();
        self.items.push(item);
        Some(v)
    }

    pub fn insert(&mut self, key: i64, value: T) {
        self.items.retain(|(k, _)| *k != key);
        self.items.push((key, value));
        while self.items.len() > self.cap {
            self.items.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::burn::{SubtitlePosition, SubtitleSize, SubtitleStyle};
    use super::super::{MAX_CJK, MAX_LATIN, MAX_LINES};
    use super::*;

    const SIZES: [(f64, f64); 4] = [(1280.0, 720.0), (1920.0, 1080.0), (3840.0, 2160.0), (1080.0, 1920.0)];

    fn styles() -> Vec<SubtitleStyle> {
        let mut v = Vec::new();
        for size in SubtitleSize::ALL {
            for position in [SubtitlePosition::Bottom, SubtitlePosition::Top] {
                for background in [true, false] {
                    v.push(SubtitleStyle { size, position, background });
                }
            }
        }
        v
    }

    fn sample() -> Vec<Segment> {
        vec![
            Segment::new(0.0, 2.5, "Hello there, this is the first line of the demo recording"),
            Segment::new(2.5, 3.0, "   "),
            Segment::new(3.0, 9.0, "今天我们来看一下如何在录屏里直接烧录字幕，并且实时预览它的样子和位置是否合适"),
            Segment::new(
                9.0,
                12.0,
                "Mixed 中英文 text with a fairly long tail that should wrap onto two lines at least",
            ),
            Segment::new(11.0, 13.0, "overlap"),
            Segment::new(14.0, 40.0, "runs past the end of the movie"),
        ]
    }

    #[test]
    fn preview_shows_exactly_the_export_cues() {
        let segs = sample();
        for (w, h) in SIZES {
            for style in styles() {
                let layout = BurnLayout::new(w, h, style);
                let export = layout_cues(&segs, &layout, 20.0);
                let mut t = -0.5;
                while t < 21.0 {
                    let want: Vec<Cue> = export.iter().filter(|c| visible_at(c, t)).cloned().collect();
                    assert_eq!(cues_at(&segs, &layout, 20.0, t), want, "{w}x{h} {style:?} t={t}");
                    t += 0.25;
                }
            }
        }
    }

    #[test]
    fn preview_cue_boundaries_and_overlap() {
        let segs = sample();
        let layout = BurnLayout::new(1920.0, 1080.0, SubtitleStyle::default());
        assert_eq!(cues_at(&segs, &layout, 20.0, 0.0).len(), 1);
        // End is exclusive, like the export's keyframes.
        assert_eq!(cues_at(&segs, &layout, 20.0, 2.5).len(), 0);
        assert_eq!(cues_at(&segs, &layout, 20.0, 11.5).len(), 2);
        assert_eq!(cues_at(&segs, &layout, 20.0, 13.5).len(), 0);
        // Clamped to the movie.
        assert_eq!(cues_at(&segs, &layout, 20.0, 19.9).len(), 1);
        assert!(cues_at(&segs, &layout, 20.0, 25.0).is_empty());
        assert!(cues_at(&segs, &layout, 20.0, f64::NAN).is_empty());
    }

    #[test]
    fn layout_at_common_sizes() {
        // (size, expected Medium font): the shorter side × 0.05.
        let expect = [36.0, 54.0, 108.0, 54.0];
        for ((w, h), font) in SIZES.into_iter().zip(expect) {
            let l = BurnLayout::new(w, h, SubtitleStyle::default());
            assert_eq!(l.font_size, font, "{w}x{h}");
            // Budgets never exceed the SRT ones and full lines fit.
            assert!(l.max_cjk <= MAX_CJK && l.max_latin <= MAX_LATIN);
            assert!(l.text_width(&"字".repeat(l.max_cjk)) <= l.max_text_width);
            for seg in sample() {
                for cue in layout_cues(&[seg], &l, 60.0) {
                    assert!(cue.lines.len() <= MAX_LINES);
                    for line in &cue.lines {
                        assert!(l.text_width(line) <= l.max_text_width + 1e-9, "{w}x{h} {line:?}");
                    }
                    // Inside the video, centred, clear of the edges.
                    let b = cue.box_rect;
                    assert!(b.min_x() >= 0.0 && b.max_x() <= w && b.min_y() >= 0.0 && b.max_y() <= h);
                    assert!((b.mid().x - w / 2.0).abs() < 1e-9);
                    assert_eq!(b.min_y(), l.margin, "bottom margin {w}x{h}");
                }
            }
        }
        // Portrait wraps narrower than landscape at the same font size.
        let land = BurnLayout::new(1920.0, 1080.0, SubtitleStyle::default());
        let port = BurnLayout::new(1080.0, 1920.0, SubtitleStyle::default());
        assert_eq!(land.font_size, port.font_size);
        assert!(port.max_cjk < land.max_cjk);
        let text = "今天我们来看一下如何在录屏里直接烧录字幕";
        assert!(port.wrap(text).len() > land.wrap(text).len());
    }

    #[test]
    fn layout_scales_with_video_size() {
        // 4K is 2× 1080p: the same lines, and boxes twice the size in twice
        // the place, give or take pixel rounding of the paddings (≤ 4 px of
        // 3840, i.e. ~0.1 % of the frame).
        for style in styles() {
            let hd = BurnLayout::new(1920.0, 1080.0, style);
            let uhd = BurnLayout::new(3840.0, 2160.0, style);
            assert_eq!(uhd.font_size, 2.0 * hd.font_size);
            assert_eq!((uhd.max_cjk, uhd.max_latin), (hd.max_cjk, hd.max_latin));
            let segs = sample();
            let a = layout_cues(&segs, &hd, 20.0);
            let b = layout_cues(&segs, &uhd, 20.0);
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(&b) {
                assert_eq!(x.lines, y.lines);
                let r = x.box_rect;
                let twice = Rect::new(r.min_x() * 2.0, r.min_y() * 2.0, r.width() * 2.0, r.height() * 2.0);
                assert!((y.box_rect.min_x() - twice.min_x()).abs() <= 4.0, "{style:?}");
                assert!((y.box_rect.min_y() - twice.min_y()).abs() <= 4.0, "{style:?}");
                assert!((y.box_rect.width() - twice.width()).abs() <= 4.0, "{style:?}");
                assert!((y.box_rect.height() - twice.height()).abs() <= 4.0, "{style:?}");
            }
        }
        // 720p keeps the same proportions within rounding.
        let sd = BurnLayout::new(1280.0, 720.0, SubtitleStyle::default());
        let hd = BurnLayout::new(1920.0, 1080.0, SubtitleStyle::default());
        assert!((sd.font_size / 720.0 - hd.font_size / 1080.0).abs() < 1e-3);
        assert!((sd.margin / 720.0 - hd.margin / 1080.0).abs() < 1e-3);
    }

    #[test]
    fn top_position_at_every_size() {
        for (w, h) in SIZES {
            let style = SubtitleStyle { position: SubtitlePosition::Top, ..SubtitleStyle::default() };
            let l = BurnLayout::new(w, h, style);
            let (b, t) = l.frames(&["Hi".into()]);
            assert_eq!(b.max_y(), h - l.margin);
            assert!(t.min_y() >= b.min_y() && t.max_y() <= b.max_y());
        }
    }

    #[test]
    fn fit_scales_to_the_pane() {
        let f = fit(1920.0, 1080.0, 480.0, 300.0);
        assert_eq!(f.scale, 0.25);
        assert_eq!(f.frame, Rect::new(0.0, 15.0, 480.0, 270.0));
        let p = fit(1080.0, 1920.0, 480.0, 300.0);
        assert!((p.frame.height() - 300.0).abs() < 1e-9);
        assert!((p.frame.mid().x - 240.0).abs() < 1e-9);
        // Small videos scale up to fill.
        assert_eq!(fit(320.0, 180.0, 640.0, 360.0).scale, 2.0);
        assert_eq!(fit(0.0, 1080.0, 480.0, 300.0).scale, 0.0);
        assert_eq!(fit(1920.0, 1080.0, f64::NAN, 300.0).scale, 0.0);
        // A cue rect mapped through the fit stays at the same relative
        // place: the bottom margin is the same fraction of the frame.
        let l = BurnLayout::new(1920.0, 1080.0, SubtitleStyle::default());
        let (b, _) = l.frames(&["x".into()]);
        assert!((b.min_y() * f.scale / f.frame.height() - l.margin / 1080.0).abs() < 1e-9);
    }

    #[test]
    fn contents_scale_tracks_zoom() {
        assert_eq!(contents_scale(0.25, 2.0), 0.5);
        assert_eq!(contents_scale(0.0, 2.0), 1.0);
        assert_eq!(contents_scale(f64::NAN, 2.0), 1.0);
        assert_eq!(contents_scale(10.0, 2.0), 4.0);
    }

    #[test]
    fn keys_times_and_midpoints() {
        assert_eq!(frame_key(1.0), 20);
        assert_eq!(frame_key(1.02), 20);
        assert_eq!(frame_key(1.03), 21);
        assert_eq!(frame_key(-3.0), 0);
        assert_eq!(frame_key(f64::INFINITY), 0);
        assert!((key_time(21) - 1.05).abs() < 1e-12);
        assert_eq!(midpoint(&Segment::new(2.0, 4.0, "a")), 3.0);
        assert_eq!(midpoint(&Segment::new(4.0, 2.0, "a")), 4.0);
        assert_eq!(clamp_time(99.0, 10.0), 10.0 - FRAME_BUCKET / 2.0);
        assert_eq!(clamp_time(-1.0, 10.0), 0.0);
        assert_eq!(clamp_time(1.0, f64::NAN), 0.0);
        let segs = sample();
        assert_eq!(segment_at(&segs, 1.0), Some(0));
        assert_eq!(segment_at(&segs, 2.7), None); // blank segment skipped
        assert_eq!(segment_at(&segs, 11.5), Some(3));
        assert_eq!(segment_at(&segs, 13.5), None);
    }

    #[test]
    fn frame_cache_is_lru() {
        let mut c = FrameCache::new(2);
        assert!(c.is_empty());
        c.insert(1, "a");
        c.insert(2, "b");
        assert_eq!(c.get(1), Some("a")); // 1 is now newest
        c.insert(3, "c"); // evicts 2
        assert_eq!(c.get(2), None);
        assert_eq!(c.get(1), Some("a"));
        assert_eq!(c.get(3), Some("c"));
        c.insert(3, "c2");
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(3), Some("c2"));
        c.clear();
        assert!(c.is_empty());
        assert_eq!(FrameCache::<u8>::new(0).cap, 1);
    }
}
