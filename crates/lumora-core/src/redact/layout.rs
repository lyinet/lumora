//! Mapping text matches back onto the image.
//!
//! OCR gives one box per recognised run. [`ocr_lines`] groups runs into
//! reading-order lines exactly like [`crate::ocr::assemble`] and remembers
//! which chars of the joined line came from which box. A match's char range
//! is then cut out of each box it touches, proportionally by estimated
//! glyph width (CJK and full-width glyphs are about twice as wide as Latin
//! ones), and padded a little so anti-aliased edges are covered too.

use std::ops::Range;

use crate::geometry::Rect;
use crate::ocr::{is_cjk, joiner, lines, TextBox};

/// One OCR box inside a line: which chars of the line it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Char range in [`OcrLine::text`].
    pub chars: Range<usize>,
    pub rect: Rect,
}

/// A line of OCR text in reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    /// Union of the segments' boxes.
    pub rect: Rect,
    pub segments: Vec<Segment>,
}

/// Group OCR boxes into lines (reading order), keeping char ↔ box mapping.
pub fn ocr_lines(boxes: &[TextBox]) -> Vec<OcrLine> {
    lines(boxes)
        .into_iter()
        .map(|line| {
            let mut text = String::new();
            let mut count = 0usize;
            let mut segments = Vec::new();
            for b in &line.boxes {
                let t = b.text.trim();
                if !text.is_empty() {
                    let j = joiner(&text, t);
                    text.push_str(j);
                    count += j.chars().count();
                }
                let n = t.chars().count();
                segments.push(Segment { chars: count..count + n, rect: b.rect });
                text.push_str(t);
                count += n;
            }
            OcrLine { text, rect: line.rect, segments }
        })
        .collect()
}

/// Relative advance width of `c` (Latin lowercase ≈ 0.55 em).
pub fn char_weight(c: char) -> f64 {
    if is_cjk(c) {
        1.0
    } else if c == ' ' {
        0.3
    } else if matches!(c, 'i' | 'l' | 'j' | 'I' | '.' | ',' | ':' | ';' | '|' | '\'' | '!' | '1' | 'f' | 't' | 'r') {
        0.35
    } else if c.is_ascii_uppercase() || matches!(c, 'm' | 'w' | 'M' | 'W' | '@' | '%') {
        0.7
    } else if c.is_ascii_digit() {
        0.58
    } else {
        0.55
    }
}

/// The part of `rect` (a box holding `text`) covering chars `range`.
pub fn sub_rect(text: &str, rect: Rect, range: Range<usize>) -> Rect {
    let weights: Vec<f64> = text.chars().map(char_weight).collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || weights.is_empty() {
        return rect;
    }
    let end = range.end.min(weights.len());
    let start = range.start.min(end);
    let before: f64 = weights[..start].iter().sum();
    let inside: f64 = weights[start..end].iter().sum();
    let x = rect.min_x() + rect.width() * before / total;
    Rect::new(x, rect.min_y(), rect.width() * inside / total, rect.height())
}

/// Rects covering chars `range` of `line` (one per box it touches, so a
/// match spanning two runs leaves the gap between them alone), each grown
/// by `pad` × the box height.
pub fn hit_rects(line: &OcrLine, range: Range<usize>, pad: f64) -> Vec<Rect> {
    let chars: Vec<char> = line.text.chars().collect();
    line.segments
        .iter()
        .filter_map(|seg| {
            let s = range.start.max(seg.chars.start);
            let e = range.end.min(seg.chars.end);
            if s >= e {
                return None;
            }
            let text: String = chars[seg.chars.clone()].iter().collect();
            let r = sub_rect(&text, seg.rect, s - seg.chars.start..e - seg.chars.start);
            let p = seg.rect.height() * pad;
            Some(r.inset(-p, -p * 0.8))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tb(t: &str, x: f64, y: f64, w: f64) -> TextBox {
        TextBox::new(t, Rect::new(x, y, w, 20.0))
    }

    #[test]
    fn lines_track_char_ranges_per_box() {
        let lines =
            ocr_lines(&[tb("mail:", 0.0, 0.0, 50.0), tb("a@b.co", 60.0, 0.0, 60.0), tb("你好", 0.0, 40.0, 40.0)]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "mail: a@b.co");
        assert_eq!(lines[0].segments[1].chars, 6..12);
        assert_eq!(lines[1].segments[0].chars, 0..2);
    }

    #[test]
    fn sub_rects_are_proportional_and_cjk_aware() {
        let r = Rect::new(0.0, 0.0, 100.0, 20.0);
        // Uniform glyphs: the second half of "aaaa" is the right half.
        let half = sub_rect("aaaa", r, 2..4);
        assert!((half.min_x() - 50.0).abs() < 1e-9 && (half.width() - 50.0).abs() < 1e-9);
        // "ab中文": the two CJK chars take more than half the width.
        let cjk = sub_rect("ab中文", r, 2..4);
        assert!(cjk.width() > 60.0 && (cjk.max_x() - 100.0).abs() < 1e-9);
        assert_eq!(sub_rect("", r, 0..0), r);
    }

    #[test]
    fn hit_rects_split_across_boxes_and_pad() {
        let lines = ocr_lines(&[tb("call 138", 0.0, 0.0, 80.0), tb("0013 8000", 100.0, 0.0, 90.0)]);
        let line = &lines[0];
        assert_eq!(line.text, "call 138 0013 8000");
        let rects = hit_rects(line, 5..18, 0.1);
        assert_eq!(rects.len(), 2);
        assert!(rects[0].min_x() > 30.0 && rects[0].max_x() >= 80.0);
        assert!(rects[1].min_x() < 100.0 && rects[1].max_x() > 190.0);
        assert!(rects[0].min_y() < 0.0 && rects[0].max_y() > 20.0);
    }
}
