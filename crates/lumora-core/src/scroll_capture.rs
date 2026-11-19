//! Scrolling capture: stitch frames of one screen area, taken while the user
//! scrolls, into one tall image.
//!
//! Pure pixel logic on tightly packed buffers (any 1–4 byte pixel format, as
//! long as every frame uses the same one).
//!
//! ## Matching
//!
//! Each row of a frame is reduced to [`CELLS`] cells: the mean of the pixels
//! in one horizontal slice of the row. Two cells are the same when their
//! means are within [`CELL_TOLERANCE`], which absorbs anti-aliasing,
//! colour-matching and compression noise; real content that moves changes
//! them by far more.
//!
//! Between the last kept frame and a new one:
//!
//! 1. **Fixed bands.** Rows at the top and bottom that are the same in both
//!    frames (a browser toolbar, a fixed nav bar, a sticky input bar at the
//!    bottom) are the header and footer, at most a third of the frame each.
//!    Only the band between them is matched.
//! 2. **Fixed columns.** Cells that did not change down the whole band (a
//!    sidebar next to a scrolling list, a window edge) say nothing about the
//!    shift and are left out, so a fixed sidebar can't veto a scroll.
//! 3. **Shift.** Every vertical shift of the band is scored by the share of
//!    differing cells over the overlapping rows (rows that are blank in both
//!    frames don't count either way); the lowest wins, provided it is at most
//!    [`MATCH_TOLERANCE`], clearly better than not moving, backed by enough
//!    rows with content, and still matches near the edge where new rows join
//!    (a frame torn mid-redraw fails that and is skipped). Shifts of a few
//!    pixels (smooth or momentum scrolling, 2× Retina frames) are found like
//!    any other.
//!
//! ## Scroll hints
//!
//! A frame may come with a [`ScrollHint`]: the scroll-wheel deltas the user
//! made since the previous frame ([`crate::scroll_trigger`]). Summed since
//! the last kept frame and passed through a [`Calibration`] learnt from
//! earlier matches (direction, and how many pixels a point of wheel delta
//! moved the content), it gives a [`HintWindow`]: the expected direction and
//! a range of shifts. That range is searched first and a match there wins
//! (repeating content can line up at several shifts; the hint picks the one
//! the user scrolled). When nothing in it matches, every shift is searched
//! as without a hint; once the calibration is trusted, a *weak* match
//! against the scroll direction is rejected as implausible (one frame at a
//! time, never two in a row, so a wrong hint can't stall a session). Frames
//! without a hint are matched exactly as before.
//!
//! ## Composing
//!
//! The image is the header once, the stitched middle, the footer once. The
//! middle grows downwards *and* upwards: a chat that opens at its newest
//! message is captured by scrolling up. Scrolling back over rows already
//! stitched moves the view without adding anything.
//!
//! Frames that did not scroll never end a session: identical frames and
//! in-place changes (a caret, a hover, an animation) are [`Push::Unchanged`];
//! before the first scroll a frame that changed beyond that becomes the new
//! start ([`Push::Restarted`]); after it, a frame that lines up with nothing
//! is skipped ([`Push::Lost`]) and only [`LOST_LIMIT`] of them in a row give
//! up ([`Push::NoOverlap`]).

use std::fmt;

/// Default cap on the stitched height, in pixels.
pub const MAX_HEIGHT: usize = 20_000;

/// Consecutive frames that line up with nothing (after scrolling started)
/// before the session gives up with [`Push::NoOverlap`]: about 1.5–5
/// seconds, depending on how fast frames come (wheel-driven or timer).
pub const LOST_LIMIT: usize = 12;

/// Cells per row signature (fewer when the frame is narrower).
pub const CELLS: usize = 64;

/// Largest difference between two cell means (0–255) that still counts as
/// the same: anti-aliasing, colour-space rounding, compression.
pub const CELL_TOLERANCE: u8 = 6;

/// Largest share of differing cells over the overlap for a shift to match
/// (a blinking caret, a moving scrollbar thumb, a small animation).
pub const MATCH_TOLERANCE: f64 = 0.06;

/// Share of differing cells at no shift below which a frame is unchanged
/// outright.
const UNCHANGED_TOLERANCE: f64 = 0.005;

/// Share of rows that may change in place for a frame that lines up at no
/// shift to still count as "not scrolled". Scrolling moves (nearly) every
/// non-blank row; three quarters of the rows where they were is not a scroll.
const IN_PLACE_TOLERANCE: f64 = 0.25;

/// A column of cells that is the same in this share of the band's rows is
/// fixed (a sidebar) and left out of the match.
const FIXED_COLUMN: f64 = 0.97;

/// Rows with content that must line up for a match, at least.
const MIN_INFO_ROWS: usize = 3;

/// Hints smaller than this many pixels say nothing.
pub const MIN_HINT_PX: f64 = 2.0;

/// The hint window spans `expected × HINT_LOW − HINT_SLACK_LOW` to
/// `expected × HINT_HIGH + HINT_SLACK_HIGH` pixels: apps animate wheel
/// steps (part of a step lands in the next frame) and accelerate them.
pub const HINT_LOW: f64 = 0.5;
pub const HINT_HIGH: f64 = 2.0;
const HINT_SLACK_LOW: f64 = 4.0;
const HINT_SLACK_HIGH: f64 = 16.0;

/// Matches that must agree with the hints' direction before a match against
/// it can be rejected.
pub const TRUSTED_AGREEMENT: usize = 3;

/// Only an opposite-direction match at least this far from a perfect one
/// (share of differing cells) is rejected; clean matches always win.
const REJECT_MISMATCH: f64 = 0.02;

/// Only hints of at least this many pixels (or line-based ones) reject.
const REJECT_MIN_PX: f64 = 16.0;

/// What the scroll events said about one frame: the content's expected
/// shift since the previous frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollHint {
    /// Raw shift in pixels, same sign convention as [`Offset::dy`]
    /// (positive: scrolled down), before calibration. Line-based deltas
    /// (`precise == false`) are in lines and only their sign is used.
    pub dy: f64,
    /// Pixel-precise deltas (trackpad, Magic Mouse).
    pub precise: bool,
}

impl ScrollHint {
    /// From wheel deltas summed since the previous frame, at `scale` pixels
    /// per point. `NSEvent.scrollingDeltaY` is positive when the content
    /// moves down, i.e. when the view moves *up* the page: the opposite of
    /// [`Offset::dy`]. `None` without events or movement.
    pub fn from_wheel(delta: &crate::scroll_trigger::WheelDelta, scale: f64) -> Option<ScrollHint> {
        if delta.events == 0 || delta.delta_y == 0.0 || !delta.delta_y.is_finite() {
            return None;
        }
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let dy = if delta.precise { -delta.delta_y * scale } else { -delta.delta_y };
        Some(ScrollHint { dy, precise: delta.precise })
    }
}

/// What the stitcher has learnt about how hints relate to the shifts found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    /// +1, or −1 when the hints turned out to point the other way.
    pub orientation: f64,
    /// Pixels of shift per pixel of (precise) hint.
    pub gain: f64,
    /// Hinted matches whose direction agreed / disagreed (since the last
    /// flip).
    pub agree: usize,
    pub disagree: usize,
}

impl Default for Calibration {
    fn default() -> Self {
        Calibration { orientation: 1.0, gain: 1.0, agree: 0, disagree: 0 }
    }
}

impl Calibration {
    /// Directions have agreed often enough to reject against them.
    pub fn trusted(&self) -> bool {
        self.agree >= TRUSTED_AGREEMENT && self.agree >= 3 * self.disagree
    }

    /// The calibrated expected shift (pixels; sign only when not precise).
    pub fn expect(&self, hint: &ScrollHint) -> f64 {
        if hint.precise {
            hint.dy * self.orientation * self.gain
        } else {
            hint.dy * self.orientation
        }
    }

    /// Learn from a match `dy` found for a raw `hint`.
    pub fn learn(&mut self, hint: &ScrollHint, dy: isize) {
        if dy == 0 || (hint.precise && hint.dy.abs() < MIN_HINT_PX) {
            return;
        }
        let expected = hint.dy * self.orientation;
        if expected.signum() == (dy as f64).signum() {
            self.agree += 1;
            if hint.precise {
                let ratio = (dy.unsigned_abs() as f64 / hint.dy.abs()).clamp(0.25, 8.0);
                self.gain = (self.gain * 0.7 + ratio * 0.3).clamp(0.25, 8.0);
            }
        } else {
            self.disagree += 1;
            if self.disagree >= 2 && self.disagree > self.agree {
                // The hints point the other way (an inverted device, an app
                // that scrolls against the wheel): flip and start over.
                *self = Calibration { orientation: -self.orientation, ..Calibration::default() };
            }
        }
    }
}

/// The shifts a hint makes likely: `sign × (lo..=hi)` rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HintWindow {
    /// +1 scrolled down, −1 up.
    pub sign: isize,
    pub lo: usize,
    pub hi: usize,
}

impl HintWindow {
    /// The window for a calibrated `expected` shift, with shifts up to
    /// `max_shift` possible. `None` when the hint is too small or the window
    /// lies beyond `max_shift`.
    pub fn new(expected: f64, precise: bool, max_shift: usize) -> Option<HintWindow> {
        if !expected.is_finite() || max_shift == 0 || (precise && expected.abs() < MIN_HINT_PX) || expected == 0.0 {
            return None;
        }
        let sign = if expected > 0.0 { 1 } else { -1 };
        let (lo, hi) = if precise {
            let e = expected.abs();
            let lo = (e * HINT_LOW - HINT_SLACK_LOW).floor().max(1.0) as usize;
            let hi = (e * HINT_HIGH + HINT_SLACK_HIGH).ceil() as usize;
            (lo, hi.min(max_shift))
        } else {
            (1, max_shift)
        };
        (lo <= hi).then_some(HintWindow { sign, lo, hi })
    }

    pub fn contains(&self, dy: isize) -> bool {
        dy.signum() == self.sign && (self.lo..=self.hi).contains(&dy.unsigned_abs())
    }
}

/// Whether a match `dy` with `mismatch` found by the full search (outside
/// the hint window) is plausible. Rejected only when the calibration is
/// trusted, the hint is clear, the match goes against it and is not clean,
/// and the previous frame was not rejected already.
pub fn plausible(cal: &Calibration, hint: &ScrollHint, dy: isize, mismatch: f64, rejected_last: bool) -> bool {
    if rejected_last || !cal.trusted() || dy == 0 {
        return true;
    }
    let expected = cal.expect(hint);
    let clear = !hint.precise || expected.abs() >= REJECT_MIN_PX;
    let against = expected.signum() != (dy as f64).signum();
    !(clear && against && mismatch > REJECT_MISMATCH)
}

/// How a hint was used for a frame (for logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintUse {
    /// The match was found inside the hint window.
    Window,
    /// Nothing matched in the window; the full search found this one.
    Outside,
    /// The full search's match went against a trusted hint; rejected.
    Rejected,
    /// The hint gave no window (too small, beyond the frame).
    NoWindow,
}

/// One captured frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// Bytes per pixel (1 = grayscale, 4 = RGBA/BGRA).
    pub bpp: usize,
    /// `height` rows of `width * bpp` bytes, no padding.
    pub data: Vec<u8>,
}

impl Frame {
    /// `None` when `data` doesn't hold exactly `width × height × bpp` bytes.
    pub fn new(width: usize, height: usize, bpp: usize, data: Vec<u8>) -> Option<Frame> {
        (width > 0 && height > 0 && (1..=4).contains(&bpp) && data.len() == width * height * bpp).then_some(Frame {
            width,
            height,
            bpp,
            data,
        })
    }

    fn row_bytes(&self) -> usize {
        self.width * self.bpp
    }

    fn rows(&self, from: usize, to: usize) -> &[u8] {
        let n = self.row_bytes();
        &self.data[from * n..to * n]
    }
}

/// What [`ScrollStitcher::push`] did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// The first frame: the stitch starts with it.
    Started,
    /// Not scrolled: same content as the last kept frame, or changed in
    /// place (a caret, a hover highlight); dropped.
    Unchanged,
    /// Not scrolled yet and the content changed too much to be an in-place
    /// tweak (the window turned active, a page re-rendered): the stitch
    /// starts over from this frame. Never final.
    Restarted,
    /// Scrolled down: this many new rows were added at the bottom.
    Appended(usize),
    /// Scrolled up past the top of the stitch: this many new rows were added
    /// at the top.
    Prepended(usize),
    /// Scrolled within what is already stitched; nothing new.
    Moved,
    /// Added this many rows and reached the height cap: stop.
    Capped(usize),
    /// After scrolling started, nothing lines up with the last kept frame
    /// (scrolled too fast, a torn frame, the content changed); dropped.
    /// Carries how many frames in a row did this. Not final.
    Lost(usize),
    /// [`LOST_LIMIT`] frames in a row lined up with nothing: stop, keeping
    /// what is stitched.
    NoOverlap,
    /// A frame of a different size or format; dropped.
    Mismatch,
}

impl Push {
    /// The session should end after this frame.
    pub fn is_final(self) -> bool {
        matches!(self, Push::Capped(_) | Push::NoOverlap)
    }
}

/// Offset between two frames, as found by [`find_offset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offset {
    /// Rows the content moved up (positive: scrolled down; negative: up).
    pub dy: isize,
    /// Fixed rows at the top of both frames.
    pub header: usize,
    /// Fixed rows at the bottom of both frames.
    pub footer: usize,
}

/// How the last frame was matched, for logs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchInfo {
    /// The shift found (`Some(0)`: not scrolled; `None`: nothing lined up).
    pub dy: Option<isize>,
    /// Fixed rows found at the top and bottom of this pair.
    pub header: usize,
    pub footer: usize,
    /// Cells per row, and how many were left out as fixed columns.
    pub cells: usize,
    pub fixed_cells: usize,
    /// Share of cells that differ with no shift (whole frame).
    pub still: f64,
    /// Share of rows that differ with no shift.
    pub rows_changed: f64,
    /// Share of differing cells at the best shift (`None`: no candidate).
    pub mismatch: Option<f64>,
    /// The best candidate shift, even when it was rejected.
    pub candidate: Option<isize>,
    /// The calibrated hint (pixels) and how it was used; `None`: no hint.
    pub hint: Option<(isize, HintUse)>,
}

impl fmt::Display for MatchInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dy {
            Some(dy) => write!(f, "dy={dy}")?,
            None => write!(f, "dy=none")?,
        }
        write!(
            f,
            " header={} footer={} fixed_cells={}/{} still={:.3} rows_changed={:.3}",
            self.header, self.footer, self.fixed_cells, self.cells, self.still, self.rows_changed
        )?;
        if let (Some(c), Some(m)) = (self.candidate, self.mismatch) {
            write!(f, " best_shift={c} mismatch={m:.3}")?;
        }
        if let Some((h, u)) = self.hint {
            let how = match u {
                HintUse::Window => "matched in window",
                HintUse::Outside => "matched outside window",
                HintUse::Rejected => "rejected against hint",
                HintUse::NoWindow => "no window",
            };
            write!(f, " hint={h} ({how})")?;
        }
        Ok(())
    }
}

/// Per-frame cell signatures.
#[derive(Debug, Clone)]
struct Sig {
    height: usize,
    cells: usize,
    /// `height × cells` means.
    data: Vec<u8>,
}

impl Sig {
    fn of(f: &Frame) -> Sig {
        let cells = CELLS.min(f.width);
        // Leave alpha out of 4-byte pixels (it is constant on screen).
        let channels = if f.bpp == 4 { 3 } else { f.bpp };
        let bounds: Vec<(usize, usize)> =
            (0..cells).map(|c| (c * f.width / cells, (c + 1) * f.width / cells)).collect();
        let mut data = Vec::with_capacity(f.height * cells);
        let rb = f.row_bytes();
        for y in 0..f.height {
            let row = &f.data[y * rb..(y + 1) * rb];
            for &(x0, x1) in &bounds {
                let mut sum: u64 = 0;
                for px in row[x0 * f.bpp..x1 * f.bpp].chunks_exact(f.bpp) {
                    for &v in &px[..channels] {
                        sum += u64::from(v);
                    }
                }
                let n = ((x1 - x0) * channels) as u64;
                data.push(((sum + n / 2) / n) as u8);
            }
        }
        Sig { height: f.height, cells, data }
    }

    fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.cells..(y + 1) * self.cells]
    }
}

fn same(a: u8, b: u8) -> bool {
    a.abs_diff(b) <= CELL_TOLERANCE
}

fn rows_equal(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).all(|(&x, &y)| same(x, y))
}

/// A row is blank (over `cols`) when all its cells are within tolerance.
fn plain(row: &[u8], cols: &[usize]) -> bool {
    let (mut lo, mut hi) = (u8::MAX, u8::MIN);
    for &c in cols {
        lo = lo.min(row[c]);
        hi = hi.max(row[c]);
    }
    hi.saturating_sub(lo) <= CELL_TOLERANCE
}

#[derive(Debug, Clone, Copy, Default)]
struct Score {
    bad: usize,
    total: usize,
    /// Rows with content that lined up.
    info: usize,
}

impl Score {
    fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.bad as f64 / self.total as f64
        }
    }
}

struct Cmp<'a> {
    a: &'a Sig,
    b: &'a Sig,
    cols: &'a [usize],
    plain_a: Vec<bool>,
    plain_b: Vec<bool>,
}

impl Cmp<'_> {
    /// Score the row pairs `(ya, yb)`; gives up (returning `None`) once more
    /// than `limit` of the possible cells differ.
    fn score(&self, pairs: impl Iterator<Item = (usize, usize)> + Clone, limit: f64) -> Option<Score> {
        let possible = pairs.clone().count() * self.cols.len();
        let allowed = (possible as f64 * limit) as usize;
        let mut s = Score::default();
        for (ya, yb) in pairs {
            let (ra, rb) = (self.a.row(ya), self.b.row(yb));
            if self.plain_a[ya] && self.plain_b[yb] && same(ra[self.cols[0]], rb[self.cols[0]]) {
                continue; // blank in both: no evidence either way
            }
            let bad = self.cols.iter().filter(|&&c| !same(ra[c], rb[c])).count();
            s.bad += bad;
            s.total += self.cols.len();
            if bad * 8 <= self.cols.len() && !(self.plain_a[ya] && self.plain_b[yb]) {
                s.info += 1;
            }
            if s.bad > allowed {
                return None;
            }
        }
        Some(s)
    }
}

/// Fixed rows at the top and bottom of a pair.
fn fixed_bands(a: &Sig, b: &Sig) -> (usize, usize) {
    let h = a.height;
    let limit = h / 3;
    let header = (0..limit).take_while(|&y| rows_equal(a.row(y), b.row(y))).count();
    let footer = (0..limit).take_while(|&i| rows_equal(a.row(h - 1 - i), b.row(h - 1 - i))).count();
    (header, footer)
}

/// Rows that must overlap for a match, at least.
fn min_overlap(band: usize) -> usize {
    (band / 4).max(8).min(band)
}

/// A hint for one match: the raw hint summed since the last kept frame,
/// the session's calibration, and whether the previous frame was rejected.
#[derive(Debug, Clone, Copy)]
struct HintCtx {
    raw: ScrollHint,
    cal: Calibration,
    rejected_last: bool,
}

/// Match frame `b` against `a`. `fixed` is the header/footer already known
/// for the session (the band matched is never larger than it).
fn match_sigs(a: &Sig, b: &Sig, fixed: Option<(usize, usize)>, hint: Option<HintCtx>) -> (Option<Offset>, MatchInfo) {
    let h = a.height;
    let all: Vec<usize> = (0..a.cells).collect();
    let mut info = MatchInfo {
        dy: None,
        header: 0,
        footer: 0,
        cells: a.cells,
        fixed_cells: 0,
        still: 0.0,
        rows_changed: 0.0,
        mismatch: None,
        candidate: None,
        hint: None,
    };
    let still_offset = Offset { dy: 0, header: 0, footer: 0 };
    let changed_rows = (0..h).filter(|&y| !rows_equal(a.row(y), b.row(y))).count();
    let changed_cells: usize =
        (0..h).map(|y| a.row(y).iter().zip(b.row(y)).filter(|(&x, &z)| !same(x, z)).count()).sum();
    info.still = changed_cells as f64 / (h * a.cells) as f64;
    info.rows_changed = changed_rows as f64 / h as f64;
    if info.still <= UNCHANGED_TOLERANCE {
        info.dy = Some(0);
        return (Some(still_offset), info);
    }
    let in_place = info.rows_changed <= IN_PLACE_TOLERANCE;
    let fallback = |mut info: MatchInfo| {
        if in_place {
            info.dy = Some(0);
            (Some(still_offset), info)
        } else {
            (None, info)
        }
    };

    let (mut header, mut footer) = fixed_bands(a, b);
    if let Some((fh, ff)) = fixed {
        header = header.max(fh);
        footer = footer.max(ff);
    }
    info.header = header;
    info.footer = footer;
    let (top, bottom) = (header, h.saturating_sub(footer));
    let band = bottom.saturating_sub(top);
    let need = min_overlap(band);
    if band == 0 || band <= need {
        return fallback(info);
    }

    // Columns that moved somewhere in the band.
    let cols: Vec<usize> = all
        .iter()
        .copied()
        .filter(|&c| {
            let same_rows = (top..bottom).filter(|&y| same(a.row(y)[c], b.row(y)[c])).count();
            (same_rows as f64) < FIXED_COLUMN * band as f64
        })
        .collect();
    info.fixed_cells = a.cells - cols.len();
    if cols.is_empty() {
        // The band didn't move at all; whatever changed is in the bands.
        return fallback(info);
    }
    let cmp = Cmp {
        a,
        b,
        plain_a: (0..h).map(|y| plain(a.row(y), &cols)).collect(),
        plain_b: (0..h).map(|y| plain(b.row(y), &cols)).collect(),
        cols: &cols,
    };
    let still_band = cmp.score((top..bottom).map(|y| (y, y)), 1.0).unwrap_or_default().ratio();

    // The best shift among `shifts`: (ratio, info rows, |dy|, dy).
    let search = |shifts: &mut dyn Iterator<Item = isize>| {
        let mut best: Option<(f64, usize, usize, isize)> = None;
        for dy in shifts {
            let d = dy.unsigned_abs();
            let n = band - d;
            // Scrolled down by d: b[top+i] shows what a[top+i+d] showed.
            let pair = move |i: usize| if dy > 0 { (top + i + d, top + i) } else { (top + i, top + i + d) };
            // A sparse pass first rules out most shifts cheaply.
            if n > 64 && cmp.score((0..n).step_by(7).map(pair), 2.0 * MATCH_TOLERANCE).is_none() {
                continue;
            }
            let Some(s) = cmp.score((0..n).map(pair), MATCH_TOLERANCE) else { continue };
            let r = s.ratio();
            if s.info < MIN_INFO_ROWS || r > MATCH_TOLERANCE {
                continue;
            }
            let better = match best {
                None => true,
                Some((br, bi, bd, _)) => {
                    r < br - 1e-9 || ((r - br).abs() <= 1e-9 && (s.info > bi || (s.info == bi && d < bd)))
                }
            };
            if better {
                best = Some((r, s.info, d, dy));
            }
        }
        best
    };
    // Whether the best shift holds up: clearly better than not moving,
    // lines up where the new rows join, and the new rows really moved.
    let holds = |ratio: f64, dy: isize| -> bool {
        let d = dy.unsigned_abs();
        // Clearly better than not moving at all.
        if ratio > still_band * 0.5 {
            return false;
        }
        // Still lines up next to where the new rows join (a frame torn
        // mid-redraw lines up at the far end only).
        let n = band - d;
        let k = (n / 8).max(16).min(n);
        let pair = move |i: usize| if dy > 0 { (top + i + d, top + i) } else { (top + i, top + i + d) };
        let edge = if dy > 0 { cmp.score((n - k..n).map(pair), 1.0) } else { cmp.score((0..k).map(pair), 1.0) };
        if !matches!(edge, Some(e) if e.ratio() <= MATCH_TOLERANCE) {
            return false;
        }
        // The rows that would be added must have moved: rows that are where
        // they were in the last frame are a stale part of a torn frame.
        let (ext_top, ext_bottom) = match fixed {
            Some((fh, ff)) => (fh, h - ff),
            None => (top, bottom),
        };
        let fresh = if dy > 0 { ext_bottom.saturating_sub(d)..ext_bottom } else { ext_top..(ext_top + d).min(h) };
        let (mut content, mut stale) = (0usize, 0usize);
        for y in fresh {
            if cmp.plain_a[y] && cmp.plain_b[y] {
                continue;
            }
            content += 1;
            stale += usize::from(cols.iter().all(|&c| same(a.row(y)[c], b.row(y)[c])));
        }
        !(content >= 2 && stale * 2 > content)
    };
    let max_shift = band - need;
    let found = |info: &mut MatchInfo, dy: isize| {
        info.dy = Some(dy);
        Some(Offset { dy, header, footer })
    };

    // The hint's window first.
    let window = hint.as_ref().and_then(|h| {
        let expected = h.cal.expect(&h.raw);
        info.hint = Some((expected.round() as isize, HintUse::NoWindow));
        HintWindow::new(expected, h.raw.precise, max_shift)
    });
    if let Some(w) = window {
        let mut shifts = (w.lo..=w.hi).map(|d| d as isize * w.sign);
        if let Some((ratio, _, _, dy)) = search(&mut shifts) {
            if holds(ratio, dy) {
                info.candidate = Some(dy);
                info.mismatch = Some(ratio);
                if let Some((_, u)) = info.hint.as_mut() {
                    *u = HintUse::Window;
                }
                let off = found(&mut info, dy);
                return (off, info);
            }
        }
    }

    // Every shift.
    let mut all_shifts = (1..=max_shift).flat_map(|d| [d as isize, -(d as isize)]);
    let Some((ratio, _, _, dy)) = search(&mut all_shifts) else { return fallback(info) };
    info.candidate = Some(dy);
    info.mismatch = Some(ratio);
    if !holds(ratio, dy) {
        return fallback(info);
    }
    if let Some(h) = &hint {
        if window.is_some() {
            if let Some((_, u)) = info.hint.as_mut() {
                *u = HintUse::Outside;
            }
        }
        if !plausible(&h.cal, &h.raw, dy, ratio, h.rejected_last) {
            if let Some((_, u)) = info.hint.as_mut() {
                *u = HintUse::Rejected;
            }
            return fallback(info);
        }
    }
    let off = found(&mut info, dy);
    (off, info)
}

/// The shift from frame `a` to frame `b` (same size). `dy == 0` when the
/// scrolling part didn't move; `None` when nothing lines up.
pub fn find_offset(a: &Frame, b: &Frame) -> Option<Offset> {
    if a.width != b.width || a.height != b.height || a.bpp != b.bpp {
        return None;
    }
    match_sigs(&Sig::of(a), &Sig::of(b), None, None).0
}

/// Counts for the end-of-session log line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub pushed: usize,
    pub unchanged: usize,
    pub restarted: usize,
    pub appended: usize,
    pub prepended: usize,
    pub moved: usize,
    pub lost: usize,
    pub mismatched: usize,
    /// Frames that came with a scroll hint; matched inside its window;
    /// rejected as going against it.
    pub hinted: usize,
    pub hint_window: usize,
    pub hint_rejected: usize,
    /// Fixed header and footer of the stitch (0 until the first scroll).
    pub header: usize,
    pub footer: usize,
    /// Rows stitched above and below the first frame's view.
    pub rows_up: usize,
    pub rows_down: usize,
    pub width: usize,
    pub height: usize,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} frames pushed: {} appended, {} prepended, {} moved, {} unchanged, {} restarted, {} lost, {} mismatched; \
             {} hinted ({} in window, {} rejected); \
             header {} px, footer {} px, +{} px above, +{} px below; image {}×{}",
            self.pushed,
            self.appended,
            self.prepended,
            self.moved,
            self.unchanged,
            self.restarted,
            self.lost,
            self.mismatched,
            self.hinted,
            self.hint_window,
            self.hint_rejected,
            self.header,
            self.footer,
            self.rows_up,
            self.rows_down,
            self.width,
            self.height
        )
    }
}

/// Builds the tall image frame by frame.
#[derive(Debug, Clone)]
pub struct ScrollStitcher {
    max_height: usize,
    width: usize,
    bpp: usize,
    /// Fixed bands, known once the content first scrolled (`None` before).
    bands: Option<(usize, usize)>,
    header: Vec<u8>,
    footer: Vec<u8>,
    /// Rows above the first frame's view, nearest first (reversed).
    up: Vec<u8>,
    /// The first frame's view and every row below it.
    down: Vec<u8>,
    /// Where the last kept frame's band starts, in rows from the top of
    /// `down` (negative: inside `up`).
    view: isize,
    /// The last kept frame and its signature.
    last: Option<(Frame, Sig)>,
    frames: usize,
    /// Frames in a row that lined up with nothing.
    lost: usize,
    match_info: Option<MatchInfo>,
    summary: Summary,
    /// Hints summed since the last kept frame.
    hint: Option<ScrollHint>,
    calibration: Calibration,
    /// The last frame was rejected against its hint.
    rejected_last: bool,
}

impl ScrollStitcher {
    /// Stop growing at `max_height` rows ([`MAX_HEIGHT`] is a good default).
    pub fn new(max_height: usize) -> Self {
        ScrollStitcher {
            max_height: max_height.max(1),
            width: 0,
            bpp: 0,
            bands: None,
            header: Vec::new(),
            footer: Vec::new(),
            up: Vec::new(),
            down: Vec::new(),
            view: 0,
            last: None,
            frames: 0,
            lost: 0,
            match_info: None,
            summary: Summary::default(),
            hint: None,
            calibration: Calibration::default(),
            rejected_last: false,
        }
    }

    /// Frames kept (the first one plus every one that added rows).
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The content has scrolled: at least one frame added rows.
    pub fn scrolled(&self) -> bool {
        self.frames > 1
    }

    pub fn width(&self) -> usize {
        self.width
    }

    fn row_bytes(&self) -> usize {
        self.width * self.bpp
    }

    fn rows_of(&self, v: &[u8]) -> usize {
        v.len().checked_div(self.row_bytes()).unwrap_or(0)
    }

    fn total_rows(&self) -> usize {
        self.rows_of(&self.header) + self.rows_of(&self.up) + self.rows_of(&self.down) + self.rows_of(&self.footer)
    }

    pub fn height(&self) -> usize {
        self.total_rows().min(self.max_height)
    }

    pub fn bytes_per_pixel(&self) -> usize {
        self.bpp
    }

    /// How the last pushed frame was matched (for logs).
    pub fn last_match(&self) -> Option<&MatchInfo> {
        self.match_info.as_ref()
    }

    /// What the hints have taught so far.
    pub fn calibration(&self) -> Calibration {
        self.calibration
    }

    /// Counts so far (for the end-of-session log line).
    pub fn summary(&self) -> Summary {
        let mut s = self.summary;
        let (h, f) = self.bands.unwrap_or((0, 0));
        s.header = h;
        s.footer = f;
        s.rows_up = self.rows_of(&self.up);
        s.rows_down = self.rows_of(&self.down).saturating_sub(self.last.as_ref().map_or(0, |l| l.0.height - h - f));
        s.width = self.width;
        s.height = self.height();
        s
    }

    /// The stitched pixels: header, rows above, first view and rows below,
    /// footer; `height()` rows of `width() × bpp` bytes.
    pub fn image(&self) -> Vec<u8> {
        let rb = self.row_bytes();
        let mut out = Vec::with_capacity(self.height() * rb);
        out.extend_from_slice(&self.header);
        for row in self.up.chunks_exact(rb.max(1)).rev() {
            out.extend_from_slice(row);
        }
        out.extend_from_slice(&self.down);
        out.extend_from_slice(&self.footer);
        out.truncate(self.max_height * rb);
        out
    }

    /// Consume the stitcher, returning `(width, height, pixels)`.
    pub fn into_image(self) -> (usize, usize, Vec<u8>) {
        let px = self.image();
        (self.width, self.height(), px)
    }

    /// Start (or restart) the stitch from `frame`.
    fn start(&mut self, frame: Frame) -> Push {
        self.width = frame.width;
        self.bpp = frame.bpp;
        self.bands = None;
        self.header.clear();
        self.footer.clear();
        self.up.clear();
        self.down = frame.data.clone();
        self.view = 0;
        self.frames = 1;
        self.lost = 0;
        self.hint = None;
        self.rejected_last = false;
        let capped = frame.height >= self.max_height;
        let sig = Sig::of(&frame);
        self.last = Some((frame, sig));
        if capped {
            Push::Capped(self.height())
        } else {
            Push::Started
        }
    }

    /// Push a frame taken without a scroll hint.
    pub fn push(&mut self, frame: Frame) -> Push {
        self.push_hinted(frame, None)
    }

    /// Push a frame with what the scroll events since the previous frame
    /// said (see the module docs); `None` matches as [`Self::push`].
    pub fn push_hinted(&mut self, frame: Frame, hint: Option<ScrollHint>) -> Push {
        self.summary.pushed += 1;
        if let Some(h) = hint {
            self.summary.hinted += 1;
            self.hint = Some(match self.hint {
                // Line-based deltas only count by sign; mixing makes the
                // sum line-based.
                Some(acc) if acc.precise == h.precise => ScrollHint { dy: acc.dy + h.dy, precise: h.precise },
                Some(acc) => ScrollHint { dy: acc.dy.signum() + h.dy.signum(), precise: false },
                None => h,
            });
        }
        let Some((last, last_sig)) = &self.last else {
            self.match_info = None;
            return self.start(frame);
        };
        if frame.width != last.width || frame.height != last.height || frame.bpp != last.bpp {
            self.summary.mismatched += 1;
            return Push::Mismatch;
        }
        if self.total_rows() >= self.max_height {
            return Push::Capped(0);
        }
        let sig = Sig::of(&frame);
        let ctx = self.hint.filter(|h| h.dy != 0.0).map(|raw| HintCtx {
            raw,
            cal: self.calibration,
            rejected_last: self.rejected_last,
        });
        let (off, info) = match_sigs(last_sig, &sig, self.bands, ctx);
        self.match_info = Some(info);
        self.rejected_last = false;
        match info.hint.map(|h| h.1) {
            Some(HintUse::Window) => self.summary.hint_window += 1,
            Some(HintUse::Rejected) => {
                self.summary.hint_rejected += 1;
                self.rejected_last = true;
            }
            _ => {}
        }
        if let (Some(raw), Some(o)) = (ctx.map(|c| c.raw), off) {
            if o.dy != 0 {
                self.calibration.learn(&raw, o.dy);
            }
        }
        let Some(off) = off else {
            if !self.scrolled() {
                // Nothing stitched beyond the first frame: the newest
                // state is the better start (the user sees it now).
                self.summary.restarted += 1;
                self.start(frame);
                return Push::Restarted;
            }
            self.lost += 1;
            self.summary.lost += 1;
            return if self.lost >= LOST_LIMIT { Push::NoOverlap } else { Push::Lost(self.lost) };
        };
        self.lost = 0;
        if off.dy == 0 {
            // Identical, or changed in place (a caret, a hover): keep
            // comparing with the last kept frame, so small scroll steps add
            // up and a change that goes away again costs nothing.
            self.summary.unchanged += 1;
            return Push::Unchanged;
        }
        self.apply(frame, sig, off)
    }

    /// Fit `frame`, shifted by `off.dy` from the last kept frame, into the
    /// stitch.
    fn apply(&mut self, frame: Frame, sig: Sig, off: Offset) -> Push {
        let h = frame.height;
        let rb = frame.row_bytes();
        let (hd, ft) = match self.bands {
            Some(b) => b,
            None => {
                // First scroll: fix the bands and split the first frame.
                let (hd, ft) = (off.header, off.footer);
                let first = std::mem::take(&mut self.down);
                self.header = first[..hd * rb].to_vec();
                self.footer = first[(h - ft) * rb..].to_vec();
                self.down = first[hd * rb..(h - ft) * rb].to_vec();
                self.bands = Some((hd, ft));
                (hd, ft)
            }
        };
        let band = h - hd - ft;
        let new_view = self.view + off.dy;
        let down_rows = self.rows_of(&self.down) as isize;
        let up_rows = self.rows_of(&self.up) as isize;
        let room = self.max_height.saturating_sub(self.total_rows());
        let result = if new_view + band as isize > down_rows {
            // New rows below. The footer comes from the lowest view.
            let new = (new_view + band as isize - down_rows) as usize;
            let take = new.min(room);
            let from = hd + (down_rows - new_view) as usize;
            self.down.extend_from_slice(frame.rows(from, from + take));
            self.footer = frame.rows(h - ft, h).to_vec();
            self.summary.appended += 1;
            self.frames += 1;
            if take < new || take == room {
                Push::Capped(take)
            } else {
                Push::Appended(take)
            }
        } else if new_view < -up_rows {
            // New rows above. The header comes from the highest view.
            let new = (-up_rows - new_view) as usize;
            let take = new.min(room);
            for k in 0..take {
                let y = hd + (-up_rows - 1 - k as isize - new_view) as usize;
                self.up.extend_from_slice(frame.rows(y, y + 1));
            }
            self.header = frame.rows(0, hd).to_vec();
            self.summary.prepended += 1;
            self.frames += 1;
            if take < new || take == room {
                Push::Capped(take)
            } else {
                Push::Prepended(take)
            }
        } else {
            self.summary.moved += 1;
            Push::Moved
        };
        self.view = new_view;
        self.last = Some((frame, sig));
        self.hint = None;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A "page": every row has a distinct grayscale pattern.
    fn page(height: usize, width: usize) -> Vec<Vec<u8>> {
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| {
                        // Blank every 5th row, like line spacing.
                        if y % 5 == 4 {
                            255
                        } else {
                            ((y * 31 + x * 7 + (y * x) % 13) % 251) as u8
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// A sticky header row `y`: a bar with a few "icons" (not blank).
    fn header_row(y: usize, w: usize) -> Vec<u8> {
        (0..w).map(|x| if x % 9 < 2 { 230 } else { 20 + y as u8 }).collect()
    }

    /// A sticky footer row, `i` rows from the bottom (an input bar).
    fn footer_row(i: usize, w: usize) -> Vec<u8> {
        (0..w).map(|x| (x % 3) as u8 * 40 + i as u8).collect()
    }

    /// The viewport of `page` scrolled to `scroll`, with sticky `header` and
    /// `footer` rows of their own.
    fn view(page: &[Vec<u8>], scroll: usize, h: usize, header: usize, footer: usize) -> Frame {
        let w = page[0].len();
        let mut data = Vec::new();
        for y in 0..h {
            if y < header {
                data.extend(header_row(y, w));
            } else if y >= h - footer {
                data.extend(footer_row(h - y, w));
            } else {
                data.extend_from_slice(&page[scroll + y - header]);
            }
        }
        Frame::new(w, h, 1, data).unwrap()
    }

    /// Header once, page rows `from..to + band`, footer once.
    fn expected(page: &[Vec<u8>], from: usize, to: usize, h: usize, header: usize, footer: usize) -> Vec<u8> {
        let w = page[0].len();
        let mut out = Vec::new();
        for y in 0..header {
            out.extend(header_row(y, w));
        }
        for row in &page[from..to + h - header - footer] {
            out.extend_from_slice(row);
        }
        for y in h - footer..h {
            out.extend(footer_row(h - y, w));
        }
        out
    }

    /// Deterministic ±`amp` noise on every byte (anti-aliasing, compression).
    fn noisy(mut f: Frame, seed: u32, amp: u8) -> Frame {
        let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        for v in &mut f.data {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s % (2 * u32::from(amp) + 1)) as i32 - i32::from(amp);
            *v = (i32::from(*v) + n).clamp(0, 255) as u8;
        }
        f
    }

    #[test]
    fn plain_scroll_stitches_exactly() {
        let p = page(400, 32);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        assert_eq!(s.push(view(&p, 0, 100, 0, 0)), Push::Started);
        assert_eq!(s.push(view(&p, 37, 100, 0, 0)), Push::Appended(37));
        assert_eq!(s.push(view(&p, 37, 100, 0, 0)), Push::Unchanged);
        assert_eq!(s.push(view(&p, 90, 100, 0, 0)), Push::Appended(53));
        assert_eq!(s.push(view(&p, 160, 100, 0, 0)), Push::Appended(70));
        assert_eq!(s.frames(), 4);
        assert_eq!(s.height(), 260);
        assert_eq!(s.image(), expected(&p, 0, 160, 100, 0, 0));
    }

    #[test]
    fn sticky_header_and_footer_appear_once() {
        let p = page(400, 24);
        let (h, hd, ft) = (120, 14, 9);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, h, hd, ft));
        let off = find_offset(&view(&p, 0, h, hd, ft), &view(&p, 25, h, hd, ft)).unwrap();
        assert_eq!(off, Offset { dy: 25, header: hd, footer: ft });
        assert_eq!(s.push(view(&p, 25, h, hd, ft)), Push::Appended(25));
        assert_eq!(s.push(view(&p, 60, h, hd, ft)), Push::Appended(35));
        assert_eq!(s.height(), h + 60);
        assert_eq!(s.image(), expected(&p, 0, 60, h, hd, ft));
        let sum = s.summary();
        assert_eq!((sum.header, sum.footer, sum.rows_down, sum.appended), (hd, ft, 60, 2));
    }

    #[test]
    fn sticky_bands_with_many_offsets_including_tiny_ones() {
        // A chat page: fixed top nav (30 rows), fixed input bar at the
        // bottom (40 rows), a scrolling list between. Momentum scrolling
        // delivers steps of a few pixels, then bigger ones, then a coast.
        let p = page(1200, 48);
        let (h, hd, ft) = (200, 30, 40);
        let steps = [1usize, 2, 3, 1, 5, 8, 13, 40, 90, 97, 60, 21, 7, 3, 2, 1, 1];
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, h, hd, ft));
        let mut at = 0;
        for d in steps {
            at += d;
            assert_eq!(s.push(view(&p, at, h, hd, ft)), Push::Appended(d), "step {d} to {at}");
        }
        assert_eq!(s.image(), expected(&p, 0, at, h, hd, ft));
        assert_eq!(s.frames(), steps.len() + 1);
    }

    #[test]
    fn noise_from_antialiasing_or_compression_is_tolerated() {
        let p = page(800, 48);
        let (h, hd, ft) = (160, 20, 24);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(noisy(view(&p, 0, h, hd, ft), 1, 2));
        let mut at = 0;
        for (i, d) in [3usize, 17, 44, 2, 61].into_iter().enumerate() {
            at += d;
            let r = s.push(noisy(view(&p, at, h, hd, ft), i as u32 + 2, 2));
            assert_eq!(r, Push::Appended(d), "step {i}: {:?}", s.last_match());
        }
        // Noise only, no scroll: unchanged.
        assert_eq!(s.push(noisy(view(&p, at, h, hd, ft), 99, 2)), Push::Unchanged);
        assert_eq!(s.height(), h + at);
    }

    #[test]
    fn chat_opened_at_its_newest_message_is_captured_by_scrolling_up() {
        // The user's case: a chat web app opens scrolled to the bottom;
        // the history is above. v0.6.2 dropped every upward frame
        // (ScrolledUp), so the count stayed at 1.
        let p = page(900, 40);
        let (h, hd, ft) = (180, 24, 36);
        let bottom = 900 - (h - hd - ft);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, bottom, h, hd, ft));
        let mut at = bottom;
        for d in [4usize, 30, 75, 2, 88, 51] {
            at -= d;
            assert_eq!(s.push(view(&p, at, h, hd, ft)), Push::Prepended(d), "to {at}");
        }
        assert_eq!(s.frames(), 7);
        assert_eq!(s.image(), expected(&p, at, bottom, h, hd, ft));
    }

    #[test]
    fn scrolling_back_over_stitched_rows_adds_nothing() {
        let p = page(600, 32);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 100, 100, 10, 10));
        assert_eq!(s.push(view(&p, 140, 100, 10, 10)), Push::Appended(40));
        assert_eq!(s.push(view(&p, 120, 100, 10, 10)), Push::Moved);
        assert_eq!(s.push(view(&p, 105, 100, 10, 10)), Push::Moved);
        assert_eq!(s.push(view(&p, 80, 100, 10, 10)), Push::Prepended(20));
        assert_eq!(s.push(view(&p, 140, 100, 10, 10)), Push::Moved);
        assert_eq!(s.push(view(&p, 200, 100, 10, 10)), Push::Appended(60));
        assert_eq!(s.image(), expected(&p, 80, 200, 100, 10, 10));
    }

    #[test]
    fn retina_frames_with_odd_pixel_offsets() {
        // 2× frames: every logical row is two device rows, and scroll steps
        // land on any device row.
        let logical = page(500, 30);
        let p: Vec<Vec<u8>> = logical
            .iter()
            .flat_map(|r| {
                let wide: Vec<u8> = r.iter().flat_map(|&v| [v, v]).collect();
                [wide.clone(), wide]
            })
            .collect();
        let (h, hd, ft) = (240, 28, 44);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, h, hd, ft));
        let mut at = 0;
        for d in [1usize, 3, 7, 2, 33, 101, 5] {
            at += d;
            assert_eq!(s.push(view(&p, at, h, hd, ft)), Push::Appended(d), "to {at}: {:?}", s.last_match());
        }
        assert_eq!(s.image(), expected(&p, 0, at, h, hd, ft));
    }

    #[test]
    fn a_fixed_sidebar_does_not_block_the_match() {
        // A chat web app captured with its sidebar: the left quarter (a
        // conversation list) never moves while the messages scroll.
        let p = page(700, 48);
        let w = 64;
        let side = |y: usize| -> Vec<u8> { (0..16).map(|x| ((y * 13 + x * 5) % 200) as u8 + 30).collect() };
        let frame = |at: usize| {
            let f = view(&p, at, 150, 20, 30);
            let mut data = Vec::new();
            for y in 0..150 {
                data.extend(side(y));
                data.extend_from_slice(&f.data[y * 48..(y + 1) * 48]);
            }
            Frame::new(w, 150, 1, data).unwrap()
        };
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(frame(0));
        let mut at = 0;
        for d in [2usize, 25, 60, 4] {
            at += d;
            assert_eq!(s.push(frame(at)), Push::Appended(d), "to {at}: {:?}", s.last_match());
        }
        assert!(s.last_match().unwrap().fixed_cells >= 16);
        // The message column is exact.
        let img = s.image();
        let exp = expected(&p, 0, at, 150, 20, 30);
        for y in 0..s.height() {
            assert_eq!(&img[y * w + 16..(y + 1) * w], &exp[y * 48..(y + 1) * 48], "row {y}");
        }
    }

    #[test]
    fn overlay_scrollbar_appearing_and_moving_is_tolerated() {
        // macOS overlay scrollbars fade in when scrolling starts and the
        // thumb moves with the content: the rightmost columns differ on
        // many rows of every pair.
        let p = page(900, 64);
        let frame = |at: usize, bar: bool| {
            let mut f = view(&p, at, 200, 20, 20);
            if bar {
                let thumb = 20 + at * 160 / 900;
                for y in 20..180 {
                    let v = if (thumb..thumb + 40).contains(&y) { 90 } else { 200 };
                    f.data[y * 64 + 62..y * 64 + 64].fill(v);
                }
            }
            f
        };
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(frame(0, false));
        let mut at = 0;
        for d in [6usize, 30, 3, 80] {
            at += d;
            assert_eq!(s.push(frame(at, true)), Push::Appended(d), "to {at}: {:?}", s.last_match());
        }
        assert_eq!(s.height(), 200 + at);
    }

    #[test]
    fn hud_free_frames_stitch_exactly() {
        // With Lumora excluded from the capture, frames carry only the
        // page; nothing of ours ends up in the image.
        let p = page(500, 40);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 120, 16, 24));
        for at in [10usize, 45, 90] {
            assert!(matches!(s.push(view(&p, at, 120, 16, 24)), Push::Appended(_)));
        }
        assert_eq!(s.image(), expected(&p, 0, 90, 120, 16, 24));
    }

    #[test]
    fn a_torn_frame_is_skipped() {
        let p = page(800, 32);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 160, 0, 0));
        assert_eq!(s.push(view(&p, 40, 160, 0, 0)), Push::Appended(40));
        // Top half already at 80, bottom half still at 40 (mid-redraw).
        let mut torn = view(&p, 80, 160, 0, 0);
        let old = view(&p, 40, 160, 0, 0);
        torn.data[80 * 32..].copy_from_slice(&old.data[80 * 32..]);
        assert!(matches!(s.push(torn), Push::Lost(1) | Push::Unchanged), "{:?}", s.last_match());
        assert_eq!(s.push(view(&p, 80, 160, 0, 0)), Push::Appended(40));
        assert_eq!(s.image(), expected(&p, 0, 80, 160, 0, 0));
    }

    #[test]
    fn unchanged_and_caret_blink_are_dropped() {
        let p = page(200, 16);
        let a = view(&p, 10, 100, 0, 0);
        let mut b = a.clone();
        b.data[50 * 16 + 3] ^= 0xff; // one row differs: a caret
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(a.clone());
        assert_eq!(s.push(a), Push::Unchanged);
        assert_eq!(s.push(b), Push::Unchanged);
        assert_eq!(s.frames(), 1);
        assert_eq!(s.height(), 100);
    }

    #[test]
    fn scrolling_up_from_the_start_prepends() {
        let p = page(300, 16);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 50, 100, 0, 0));
        assert_eq!(find_offset(&view(&p, 50, 100, 0, 0), &view(&p, 20, 100, 0, 0)).map(|o| o.dy), Some(-30));
        assert_eq!(s.push(view(&p, 20, 100, 0, 0)), Push::Prepended(30));
        assert_eq!(s.push(view(&p, 80, 100, 0, 0)), Push::Appended(30));
        assert_eq!(s.image(), expected(&p, 20, 80, 100, 0, 0));
    }

    #[test]
    fn no_overlap_stops_and_keeps_the_stitch() {
        let p = page(600, 16);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 100, 0, 0));
        s.push(view(&p, 40, 100, 0, 0));
        let before = s.image();
        // Skipped while it might still line up again…
        for n in 1..LOST_LIMIT {
            let r = s.push(view(&p, 400, 100, 0, 0));
            assert_eq!(r, Push::Lost(n));
            assert!(!r.is_final());
        }
        // …and only a run of LOST_LIMIT stops.
        let r = s.push(view(&p, 400, 100, 0, 0));
        assert_eq!(r, Push::NoOverlap);
        assert!(r.is_final());
        assert_eq!(s.image(), before);
        assert_eq!(s.height(), 140);
    }

    #[test]
    fn a_lost_frame_does_not_break_the_stitch() {
        let p = page(600, 16);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 100, 0, 0));
        assert_eq!(s.push(view(&p, 30, 100, 0, 0)), Push::Appended(30));
        // A garbage frame (mid-redraw) in between…
        assert_eq!(s.push(view(&p, 500, 100, 0, 0)), Push::Lost(1));
        // …and the next good one lines up with the last kept frame.
        assert_eq!(s.push(view(&p, 70, 100, 0, 0)), Push::Appended(40));
        assert_eq!(s.image(), expected(&p, 0, 70, 100, 0, 0));
        // The run of lost frames was reset.
        assert_eq!(s.push(view(&p, 500, 100, 0, 0)), Push::Lost(1));
    }

    #[test]
    fn identical_consecutive_frames_never_end_the_session() {
        let p = page(300, 16);
        let f = view(&p, 0, 100, 0, 0);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        assert_eq!(s.push(f.clone()), Push::Started);
        for _ in 0..(LOST_LIMIT * 3) {
            let r = s.push(f.clone());
            assert_eq!(r, Push::Unchanged);
            assert!(!r.is_final());
        }
        assert!(!s.scrolled());
        assert_eq!((s.frames(), s.height()), (1, 100));
        // Scrolling afterwards still stitches.
        assert_eq!(s.push(view(&p, 25, 100, 0, 0)), Push::Appended(25));
        assert!(s.scrolled());
    }

    #[test]
    fn small_dynamic_changes_without_scrolling_are_unchanged() {
        let p = page(300, 16);
        let base = view(&p, 0, 100, 0, 0);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(base.clone());
        // An animated block (a spinner, a hover highlight) covering 12% of
        // the rows, in place: more than a caret, far less than a scroll.
        for frame in 1..6u8 {
            let mut f = base.clone();
            for y in 40..52 {
                for x in 2..10 {
                    f.data[y * 16 + x] = frame.wrapping_mul(37);
                }
            }
            let r = s.push(f);
            assert_eq!(r, Push::Unchanged, "frame {frame}");
            assert!(!r.is_final());
        }
        // A blinking caret on top of that, too.
        let mut caret = base.clone();
        caret.data[70 * 16 + 5] ^= 0xff;
        assert_eq!(s.push(caret), Push::Unchanged);
        assert_eq!((s.frames(), s.height()), (1, 100));
        assert_eq!(s.push(view(&p, 20, 100, 0, 0)), Push::Appended(20));
    }

    #[test]
    fn window_turning_active_before_scrolling_restarts_instead_of_stopping() {
        // The v0.6.1 bug: clicking into the chosen window (to focus it)
        // re-tints its sidebar/toolbar on every row; nothing lines up, and
        // the old stitcher answered NoOverlap on the 2nd frame, which ended
        // the session and saved the single frame.
        let p = page(400, 32);
        let tint = |mut f: Frame, v: u8| {
            for y in 0..f.height {
                f.data[y * f.width..y * f.width + 6].fill(v);
            }
            f
        };
        let inactive = tint(view(&p, 0, 100, 0, 0), 200);
        let active = tint(view(&p, 0, 100, 0, 0), 90);
        assert_eq!(find_offset(&inactive, &active), None);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(inactive);
        let r = s.push(active.clone());
        assert_eq!(r, Push::Restarted);
        assert!(!r.is_final());
        assert!(!s.scrolled());
        assert_eq!(s.image(), active.data);
        // Then the user scrolls the (now active) window: it stitches.
        assert_eq!(s.push(tint(view(&p, 30, 100, 0, 0), 90)), Push::Appended(30));
        assert_eq!(s.height(), 130);
        // Never NoOverlap before a scroll, however many changes come.
        let mut t = ScrollStitcher::new(MAX_HEIGHT);
        t.push(view(&p, 0, 100, 0, 0));
        for i in 0..(LOST_LIMIT * 2) {
            let r = t.push(view(&p, 200 + (i % 2) * 100, 100, 0, 0));
            assert!(!r.is_final(), "{r:?}");
        }
    }

    #[test]
    fn blank_frames_do_not_match_anything() {
        let blank = Frame::new(8, 60, 1, vec![255; 480]).unwrap();
        let mut other = blank.clone();
        other.data[..8 * 20].fill(0);
        // Only blank rows line up at every shift: no evidence, no match.
        assert_eq!(find_offset(&blank, &other), None);
    }

    #[test]
    fn height_is_capped() {
        let p = page(800, 8);
        let mut s = ScrollStitcher::new(180);
        s.push(view(&p, 0, 100, 0, 0));
        assert_eq!(s.push(view(&p, 50, 100, 0, 0)), Push::Appended(50));
        let r = s.push(view(&p, 100, 100, 0, 0));
        assert_eq!(r, Push::Capped(30));
        assert!(r.is_final());
        assert_eq!(s.height(), 180);
        assert_eq!(s.image(), expected(&p, 0, 100, 100, 0, 0)[..180 * 8]);
    }

    #[test]
    fn rgba_frames_and_mismatches() {
        let p = page(300, 10);
        let rgba = |f: Frame| {
            let data = f.data.iter().flat_map(|&g| [g, g / 2, 255 - g, 255]).collect();
            Frame::new(f.width, f.height, 4, data).unwrap()
        };
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(rgba(view(&p, 0, 80, 0, 0)));
        assert_eq!(s.push(rgba(view(&p, 33, 80, 0, 0))), Push::Appended(33));
        assert_eq!(s.push(view(&p, 40, 80, 0, 0)), Push::Mismatch);
        let (w, h, px) = s.into_image();
        assert_eq!((w, h, px.len()), (10, 113, 10 * 113 * 4));
        assert!(Frame::new(3, 3, 4, vec![0; 5]).is_none());
    }

    #[test]
    fn match_info_and_summary_describe_the_session() {
        let p = page(400, 32);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 100, 12, 8));
        s.push(view(&p, 30, 100, 12, 8));
        let m = *s.last_match().unwrap();
        assert_eq!((m.dy, m.header), (Some(30), 12));
        // A blank page row right above the input bar is the same in both
        // frames too; it may count as footer (harmless: it is shown once).
        assert!((8..=9).contains(&m.footer));
        assert!(m.to_string().starts_with("dy=30 header=12 footer="));
        s.push(view(&p, 30, 100, 12, 8));
        let sum = s.summary();
        assert_eq!((sum.pushed, sum.appended, sum.unchanged, sum.height), (3, 1, 1, 130));
        assert!(sum.to_string().contains("1 appended"));
    }

    /// `page` rows repeating every `period` rows (a list of identical items).
    fn periodic_page(height: usize, width: usize, period: usize) -> Vec<Vec<u8>> {
        let one = page(period, width);
        (0..height).map(|y| one[y % period].clone()).collect()
    }

    fn hint(dy: f64) -> Option<ScrollHint> {
        Some(ScrollHint { dy, precise: true })
    }

    #[test]
    fn hint_window_bounds() {
        let w = HintWindow::new(80.0, true, 500).unwrap();
        assert_eq!(w, HintWindow { sign: 1, lo: 36, hi: 176 });
        assert!(w.contains(80) && w.contains(36) && !w.contains(35) && !w.contains(-80));
        let w = HintWindow::new(-10.0, true, 500).unwrap();
        assert_eq!(w, HintWindow { sign: -1, lo: 1, hi: 36 });
        // Clamped to what the frame allows; beyond it, no window.
        assert_eq!(HintWindow::new(100.0, true, 60).unwrap().hi, 60);
        assert_eq!(HintWindow::new(400.0, true, 60), None);
        // Too small to say anything.
        assert_eq!(HintWindow::new(1.5, true, 500), None);
        assert_eq!(HintWindow::new(0.0, false, 500), None);
        // Line-based: direction only.
        assert_eq!(HintWindow::new(-3.0, false, 90), Some(HintWindow { sign: -1, lo: 1, hi: 90 }));
    }

    #[test]
    fn hints_from_wheel_deltas() {
        use crate::scroll_trigger::WheelDelta;
        // Fingers up on a trackpad (natural): deltaY < 0, the view moves down
        // the page: dy > 0, in pixels at 2×.
        let d = WheelDelta { delta_y: -12.5, precise: true, events: 3 };
        assert_eq!(ScrollHint::from_wheel(&d, 2.0), Some(ScrollHint { dy: 25.0, precise: true }));
        let d = WheelDelta { delta_y: 2.0, precise: false, events: 1 };
        assert_eq!(ScrollHint::from_wheel(&d, 2.0), Some(ScrollHint { dy: -2.0, precise: false }));
        assert_eq!(ScrollHint::from_wheel(&WheelDelta::default(), 2.0), None);
    }

    #[test]
    fn calibration_learns_gain_and_flips_when_hints_point_the_other_way() {
        let mut c = Calibration::default();
        let h = ScrollHint { dy: 20.0, precise: true };
        for _ in 0..3 {
            c.learn(&h, 40);
        }
        assert!(c.trusted());
        assert!(c.gain > 1.5 && c.gain < 2.0, "{c:?}");
        assert!(c.expect(&h) > 30.0);
        // Hints the other way round from the start: flips after two.
        let mut c = Calibration::default();
        c.learn(&h, -20);
        assert_eq!(c.orientation, 1.0);
        c.learn(&h, -20);
        assert_eq!((c.orientation, c.agree, c.disagree), (-1.0, 0, 0));
        assert!(c.expect(&h) < 0.0);
        // Tiny precise hints and no shift teach nothing.
        c.learn(&ScrollHint { dy: 1.0, precise: true }, 5);
        c.learn(&h, 0);
        assert_eq!((c.agree, c.disagree), (0, 0));
    }

    #[test]
    fn only_weak_matches_against_a_trusted_hint_are_implausible() {
        let h = ScrollHint { dy: 60.0, precise: true };
        let fresh = Calibration::default();
        let trusted = Calibration { agree: 5, ..Calibration::default() };
        // Untrusted: anything goes.
        assert!(plausible(&fresh, &h, -60, 0.05, false));
        // Trusted, against the hint, weak: rejected once, not twice in a row.
        assert!(!plausible(&trusted, &h, -60, 0.05, false));
        assert!(plausible(&trusted, &h, -60, 0.05, true));
        // A clean match always wins.
        assert!(plausible(&trusted, &h, -60, 0.0, false));
        // With the hint, or a small hint: fine.
        assert!(plausible(&trusted, &h, 120, 0.05, false));
        assert!(plausible(&trusted, &ScrollHint { dy: 8.0, precise: true }, -8, 0.05, false));
        // Line-based hints are clear by direction.
        assert!(!plausible(&trusted, &ScrollHint { dy: 3.0, precise: false }, -8, 0.05, false));
    }

    #[test]
    fn a_hint_picks_the_scrolled_shift_in_repeating_content() {
        let p = periodic_page(600, 32, 64);
        // Without a hint the smallest shift that lines up wins (16, not 80).
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 200, 0, 0));
        assert_eq!(s.push(view(&p, 80, 200, 0, 0)), Push::Appended(16));
        // With the hint, the 80 the user scrolled.
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push_hinted(view(&p, 0, 200, 0, 0), None);
        assert_eq!(s.push_hinted(view(&p, 80, 200, 0, 0), hint(80.0)), Push::Appended(80));
        let m = s.last_match().unwrap();
        assert_eq!(m.hint, Some((80, HintUse::Window)));
        assert!(m.to_string().contains("hint=80 (matched in window)"));
        assert_eq!(s.image(), expected(&p, 0, 80, 200, 0, 0));
        assert_eq!((s.summary().hinted, s.summary().hint_window), (1, 1));
    }

    #[test]
    fn hints_add_up_over_frames_that_did_not_move() {
        let p = periodic_page(600, 32, 64);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 200, 0, 0));
        // The app hasn't drawn the first step yet.
        assert_eq!(s.push_hinted(view(&p, 0, 200, 0, 0), hint(40.0)), Push::Unchanged);
        assert_eq!(s.push_hinted(view(&p, 80, 200, 0, 0), hint(40.0)), Push::Appended(80));
        assert_eq!(s.last_match().unwrap().hint, Some((80, HintUse::Window)));
    }

    #[test]
    fn hinted_sessions_stitch_like_unhinted_ones() {
        let p = page(600, 32);
        let (h, hd, ft) = (150, 12, 8);
        let steps = [0usize, 30, 75, 75, 140, 200, 260, 330];
        let mut plain = ScrollStitcher::new(MAX_HEIGHT);
        let mut hinted = ScrollStitcher::new(MAX_HEIGHT);
        let mut prev = 0;
        for &y in &steps {
            plain.push(view(&p, y, h, hd, ft));
            // Hints a bit off (apps accelerate and animate).
            let d = (y as f64 - prev as f64) * 0.8;
            hinted.push_hinted(view(&p, y, h, hd, ft), if d == 0.0 { None } else { hint(d) });
            prev = y;
        }
        assert_eq!(plain.image(), hinted.image());
        assert_eq!(hinted.image(), expected(&p, 0, 330, h, hd, ft));
        assert!(hinted.calibration().trusted());
    }

    #[test]
    fn hints_pointing_the_wrong_way_still_stitch_and_get_flipped() {
        let p = page(800, 32);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 150, 0, 0));
        let mut y = 0;
        for _ in 0..6 {
            y += 40;
            let push = s.push_hinted(view(&p, y, 150, 0, 0), hint(-40.0));
            assert_eq!(push, Push::Appended(40));
        }
        assert_eq!(s.calibration().orientation, -1.0);
        assert_eq!(s.last_match().unwrap().hint, Some((40, HintUse::Window)));
        assert_eq!(s.summary().hint_rejected, 0);
        assert_eq!(s.image(), expected(&p, 0, y, 150, 0, 0));
    }
}
