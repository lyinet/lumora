//! Scrolling capture: stitch frames of one screen area, taken while the user
//! scrolls, into one tall image.
//!
//! Pure pixel logic on tightly packed buffers (any 1–4 byte pixel format, as
//! long as every frame uses the same one). Each frame is reduced to one hash
//! per row; the vertical offset between consecutive frames is the shift that
//! lines the most rows up. Rows that stay put at the top and bottom of both
//! frames (a sticky header or footer) are left out of the match and kept
//! once: the header from the first frame, the footer from the last.

/// Default cap on the stitched height, in pixels.
pub const MAX_HEIGHT: usize = 20_000;

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

    fn row(&self, y: usize) -> &[u8] {
        let n = self.width * self.bpp;
        &self.data[y * n..(y + 1) * n]
    }
}

/// What [`ScrollStitcher::push`] did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// The first frame: the stitch starts with it.
    Started,
    /// Same content as the last kept frame (or not scrolled); dropped.
    Unchanged,
    /// Scrolled back up; dropped (the stitch only grows downwards).
    ScrolledUp,
    /// Scrolled down: this many new rows were appended.
    Appended(usize),
    /// Appended this many rows and reached the height cap: stop.
    Capped(usize),
    /// Nothing lines up with the last frame (scrolled too fast, or the
    /// content changed): stop, keeping what is stitched.
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
    /// Sticky rows at the top of both frames.
    pub header: usize,
    /// Sticky rows at the bottom of both frames.
    pub footer: usize,
}

/// Row signature: a hash of the row's bytes plus whether every pixel in it is
/// the same (a blank row says nothing about where it belongs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    hash: u64,
    plain: bool,
}

fn rows(f: &Frame) -> Vec<Row> {
    (0..f.height)
        .map(|y| {
            let r = f.row(y);
            // FNV-1a over 8-byte words (fast enough for a few MB per frame).
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for c in r.chunks(8) {
                let mut w = [0u8; 8];
                w[..c.len()].copy_from_slice(c);
                h ^= u64::from_le_bytes(w);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            let first = &r[..f.bpp];
            Row { hash: h, plain: r.chunks(f.bpp).all(|p| p == first) }
        })
        .collect()
}

/// Rows that must line up for a match, at least.
fn min_overlap(band: usize) -> usize {
    (band / 4).max(8).min(band)
}

/// Share of the overlapping rows allowed to differ (a blinking caret, a
/// hover highlight, an animated spinner).
const MISMATCH_TOLERANCE: f64 = 0.04;

/// Sticky bands are at most a third of the frame each.
fn sticky_limit(h: usize) -> usize {
    h / 3
}

fn find_offset_rows(a: &[Row], b: &[Row]) -> Option<Offset> {
    let h = a.len();
    if h != b.len() || h == 0 {
        return None;
    }
    // Nothing moved, or only a few rows changed in place (a caret blink).
    let changed = a.iter().zip(b).filter(|(x, y)| x != y).count();
    if changed as f64 / h as f64 <= MISMATCH_TOLERANCE {
        return Some(Offset { dy: 0, header: 0, footer: 0 });
    }
    let limit = sticky_limit(h);
    let header = a.iter().zip(b).take(limit).take_while(|(x, y)| x == y).count();
    let footer = a.iter().rev().zip(b.iter().rev()).take(limit).take_while(|(x, y)| x == y).count();
    let (top, bottom) = (header, h - footer);
    let band = bottom.saturating_sub(top);
    let need = min_overlap(band);
    if band < need || band == 0 {
        return None;
    }
    let max_shift = band - need;
    // (mismatch ratio, informative matches, |dy|) — lower ratio, more
    // informative matches, smaller shift win.
    let mut best: Option<(f64, usize, usize, isize)> = None;
    for d in 1..=max_shift {
        for dy in [d as isize, -(d as isize)] {
            let n = band - d;
            let allowed = (n as f64 * MISMATCH_TOLERANCE) as usize;
            let (mut bad, mut info) = (0usize, 0usize);
            for i in 0..n {
                // Scrolled down by d: b[top+i] shows what a[top+i+d] showed.
                let (ra, rb) = if dy > 0 { (a[top + i + d], b[top + i]) } else { (a[top + i], b[top + i + d]) };
                if ra.hash == rb.hash {
                    info += usize::from(!ra.plain);
                } else {
                    bad += 1;
                    if bad > allowed {
                        break;
                    }
                }
            }
            let ratio = bad as f64 / n as f64;
            if ratio > MISMATCH_TOLERANCE || info == 0 {
                continue;
            }
            let better = match best {
                None => true,
                Some((r, i, ad, _)) => {
                    ratio < r - 1e-9 || ((ratio - r).abs() <= 1e-9 && (info > i || (info == i && d < ad)))
                }
            };
            if better {
                best = Some((ratio, info, d, dy));
            }
        }
    }
    best.map(|(_, _, _, dy)| Offset { dy, header, footer })
}

/// The shift from frame `a` to frame `b` (same size). `dy == 0` when the
/// scrolling part didn't move; `None` when nothing lines up.
pub fn find_offset(a: &Frame, b: &Frame) -> Option<Offset> {
    if a.width != b.width || a.height != b.height || a.bpp != b.bpp {
        return None;
    }
    find_offset_rows(&rows(a), &rows(b))
}

/// Builds the tall image frame by frame.
#[derive(Debug, Clone)]
pub struct ScrollStitcher {
    max_height: usize,
    width: usize,
    bpp: usize,
    /// The stitched rows so far.
    out: Vec<u8>,
    /// The last kept frame and its row signatures.
    last: Option<(Frame, Vec<Row>)>,
    /// Rows at the end of `out` that are the last frame's bottom rows.
    tail: usize,
    frames: usize,
}

impl ScrollStitcher {
    /// Stop growing at `max_height` rows ([`MAX_HEIGHT`] is a good default).
    pub fn new(max_height: usize) -> Self {
        ScrollStitcher {
            max_height: max_height.max(1),
            width: 0,
            bpp: 0,
            out: Vec::new(),
            last: None,
            tail: 0,
            frames: 0,
        }
    }

    /// Frames kept (the first one plus every one that added rows).
    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        if self.width == 0 {
            0
        } else {
            self.out.len() / (self.width * self.bpp)
        }
    }

    pub fn bytes_per_pixel(&self) -> usize {
        self.bpp
    }

    /// The stitched pixels: `height()` rows of `width() × bpp` bytes.
    pub fn pixels(&self) -> &[u8] {
        &self.out
    }

    /// Consume the stitcher, returning `(width, height, pixels)`.
    pub fn into_image(self) -> (usize, usize, Vec<u8>) {
        let h = self.height();
        (self.width, h, self.out)
    }

    pub fn push(&mut self, frame: Frame) -> Push {
        let row_bytes = frame.width * frame.bpp;
        let Some((last, last_rows)) = &self.last else {
            self.width = frame.width;
            self.bpp = frame.bpp;
            let keep = frame.height.min(self.max_height);
            self.out = frame.data[..keep * row_bytes].to_vec();
            self.tail = keep;
            self.frames = 1;
            let rows = rows(&frame);
            self.last = Some((frame, rows));
            return if keep < self.last.as_ref().map_or(0, |l| l.0.height) {
                Push::Capped(keep)
            } else {
                Push::Started
            };
        };
        if frame.width != last.width || frame.height != last.height || frame.bpp != last.bpp {
            return Push::Mismatch;
        }
        let new_rows = rows(&frame);
        let Some(off) = find_offset_rows(last_rows, &new_rows) else { return Push::NoOverlap };
        let h = frame.height;
        if off.dy == 0 {
            return Push::Unchanged;
        }
        if off.dy < 0 {
            return Push::ScrolledUp;
        }
        let d = off.dy as usize;
        // The footer currently ends `out`; drop it, add the d new rows of
        // the scrolling band, then the footer again (from the new frame).
        let footer = off.footer.min(self.tail);
        self.out.truncate(self.out.len() - footer * row_bytes);
        let band_end = h - off.footer;
        let room = self.max_height.saturating_sub(self.height() + footer);
        let take = d.min(room);
        let start = band_end - d;
        self.out.extend_from_slice(&frame.data[start * row_bytes..(start + take) * row_bytes]);
        self.out.extend_from_slice(&frame.data[(h - footer) * row_bytes..h * row_bytes]);
        self.tail = (self.tail + d).min(h - off.header);
        self.frames += 1;
        self.last = Some((frame, new_rows));
        if take < d || self.height() >= self.max_height {
            Push::Capped(take)
        } else {
            Push::Appended(take)
        }
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

    /// The viewport of `page` scrolled to `scroll`, with sticky `header` and
    /// `footer` rows of their own.
    fn view(page: &[Vec<u8>], scroll: usize, h: usize, header: usize, footer: usize) -> Frame {
        let w = page[0].len();
        let mut data = Vec::new();
        for y in 0..h {
            if y < header {
                data.extend(std::iter::repeat_n(10 + y as u8, w));
            } else if y >= h - footer {
                data.extend((0..w).map(|x| (x % 3) as u8 * 40 + (h - y) as u8));
            } else {
                data.extend_from_slice(&page[scroll + y - header]);
            }
        }
        Frame::new(w, h, 1, data).unwrap()
    }

    fn expected(page: &[Vec<u8>], scroll_end: usize, h: usize, header: usize, footer: usize) -> Vec<u8> {
        let w = page[0].len();
        let first = view(page, 0, h, header, footer);
        let mut out = first.data[..header * w].to_vec();
        for row in &page[..scroll_end + h - header - footer] {
            out.extend_from_slice(row);
        }
        let last = view(page, scroll_end, h, header, footer);
        out.extend_from_slice(&last.data[(h - footer) * w..]);
        out
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
        assert_eq!(s.pixels(), &expected(&p, 160, 100, 0, 0)[..]);
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
        assert_eq!(s.pixels(), &expected(&p, 60, h, hd, ft)[..]);
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
    fn scrolling_up_is_ignored() {
        let p = page(300, 16);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 50, 100, 0, 0));
        assert_eq!(find_offset(&view(&p, 50, 100, 0, 0), &view(&p, 20, 100, 0, 0)).map(|o| o.dy), Some(-30));
        assert_eq!(s.push(view(&p, 20, 100, 0, 0)), Push::ScrolledUp);
        // Still compared against the last kept frame.
        assert_eq!(s.push(view(&p, 80, 100, 0, 0)), Push::Appended(30));
    }

    #[test]
    fn no_overlap_stops_and_keeps_the_stitch() {
        let p = page(600, 16);
        let mut s = ScrollStitcher::new(MAX_HEIGHT);
        s.push(view(&p, 0, 100, 0, 0));
        s.push(view(&p, 40, 100, 0, 0));
        let before = s.pixels().to_vec();
        let r = s.push(view(&p, 400, 100, 0, 0));
        assert_eq!(r, Push::NoOverlap);
        assert!(r.is_final());
        assert_eq!(s.pixels(), &before[..]);
        assert_eq!(s.height(), 140);
    }

    #[test]
    fn blank_frames_do_not_match_anything() {
        let blank = Frame::new(8, 60, 1, vec![255; 480]).unwrap();
        let mut other = blank.clone();
        other.data[..8 * 20].fill(0);
        // Only plain rows line up at every shift: no evidence, no match.
        assert_eq!(find_offset_rows(&rows(&blank), &rows(&other)), None);
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
        assert_eq!(s.pixels(), &expected(&p, 100, 100, 0, 0)[..180 * 8]);
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
}
