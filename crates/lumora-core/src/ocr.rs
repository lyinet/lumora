//! Turning OCR observations into clipboard text.
//!
//! Vision returns one observation per detected text run, each with a
//! bounding box, in no guaranteed order. [`assemble`] sorts them into
//! reading order (lines top → bottom, runs left → right), joins runs with a
//! space unless both sides are CJK (Chinese and Japanese don't use spaces),
//! and leaves a blank line where the vertical gap suggests a new paragraph.

use crate::geometry::Rect;

/// Languages requested from Vision, in priority order.
pub const RECOGNITION_LANGUAGES: [&str; 4] = ["en-US", "zh-Hans", "zh-Hant", "ja-JP"];

/// One recognised run of text. `rect` is in any y-down space (Vision's
/// normalised boxes, flipped by the caller).
#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
    pub text: String,
    pub rect: Rect,
}

impl TextBox {
    pub fn new(text: impl Into<String>, rect: Rect) -> Self {
        TextBox { text: text.into(), rect }
    }
}

/// Whether `c` belongs to a script written without spaces between words.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F   // CJK punctuation
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF // Extension A
        | 0x4E00..=0x9FFF // Unified ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // Compatibility ideographs
        | 0xFF00..=0xFFEF // Full-width forms
    )
}

fn joiner(left: &str, right: &str) -> &'static str {
    match (left.chars().last(), right.chars().next()) {
        (Some(a), Some(b)) if is_cjk(a) && is_cjk(b) => "",
        (Some(a), Some(b)) if is_cjk(a) && b.is_ascii_punctuation() => "",
        _ => " ",
    }
}

/// Vertical overlap of two boxes as a fraction of the shorter one.
fn vertical_overlap(a: &Rect, b: &Rect) -> f64 {
    let top = a.min_y().max(b.min_y());
    let bottom = a.max_y().min(b.max_y());
    let shorter = a.height().min(b.height());
    if shorter <= 0.0 {
        return 0.0;
    }
    ((bottom - top) / shorter).max(0.0)
}

struct Line<'a> {
    rect: Rect,
    boxes: Vec<&'a TextBox>,
}

/// Group observations into lines in reading order.
fn lines(boxes: &[TextBox]) -> Vec<Line<'_>> {
    let mut sorted: Vec<&TextBox> = boxes.iter().filter(|b| !b.text.trim().is_empty()).collect();
    sorted.sort_by(|a, b| a.rect.mid().y.total_cmp(&b.rect.mid().y));
    let mut out: Vec<Line> = Vec::new();
    for b in sorted {
        // Join the line it overlaps most (at least half its height).
        let best = out
            .iter_mut()
            .map(|l| (vertical_overlap(&l.rect, &b.rect), l))
            .filter(|(o, _)| *o >= 0.5)
            .max_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            Some((_, line)) => {
                line.rect = line.rect.union(&b.rect);
                line.boxes.push(b);
            }
            None => out.push(Line { rect: b.rect, boxes: vec![b] }),
        }
    }
    for l in &mut out {
        l.boxes.sort_by(|a, b| a.rect.min_x().total_cmp(&b.rect.min_x()));
    }
    out.sort_by(|a, b| a.rect.min_y().total_cmp(&b.rect.min_y()));
    out
}

/// Join observations into text in reading order.
pub fn assemble(boxes: &[TextBox]) -> String {
    let lines = lines(boxes);
    if lines.is_empty() {
        return String::new();
    }
    let mut heights: Vec<f64> = lines.iter().map(|l| l.rect.height()).collect();
    heights.sort_by(f64::total_cmp);
    let median = heights[heights.len() / 2];
    let mut out = String::new();
    let mut prev: Option<&Rect> = None;
    for line in &lines {
        if let Some(p) = prev {
            let gap = line.rect.min_y() - p.max_y();
            out.push_str(if gap > median * 0.9 { "\n\n" } else { "\n" });
        }
        let mut text = String::new();
        for b in &line.boxes {
            let t = b.text.trim();
            if !text.is_empty() {
                text.push_str(joiner(&text, t));
            }
            text.push_str(t);
        }
        out.push_str(&text);
        prev = Some(&line.rect);
    }
    out
}

/// A one- or two-line preview for the toast: the first non-empty lines,
/// truncated to `max_chars` characters with an ellipsis.
pub fn preview(text: &str, max_chars: usize) -> String {
    let flat: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(2).collect();
    let joined = flat.join(" ");
    if joined.chars().count() <= max_chars {
        let more = text.lines().filter(|l| !l.trim().is_empty()).count() > flat.len();
        return if more { format!("{joined} …") } else { joined };
    }
    let cut: String = joined.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tb(t: &str, x: f64, y: f64, w: f64) -> TextBox {
        TextBox::new(t, Rect::new(x, y, w, 0.04))
    }

    #[test]
    fn sorts_into_reading_order() {
        let boxes = vec![tb("world", 0.4, 0.1, 0.2), tb("second line", 0.1, 0.15, 0.4), tb("Hello", 0.1, 0.1, 0.2)];
        assert_eq!(assemble(&boxes), "Hello world\nsecond line");
    }

    #[test]
    fn slightly_skewed_runs_share_a_line() {
        let boxes = vec![tb("a", 0.0, 0.100, 0.1), tb("b", 0.2, 0.112, 0.1), tb("c", 0.4, 0.124, 0.1)];
        assert_eq!(assemble(&boxes), "a b c");
    }

    #[test]
    fn cjk_runs_join_without_spaces() {
        let boxes = vec![tb("你好", 0.0, 0.1, 0.1), tb("世界", 0.2, 0.1, 0.1), tb("OK", 0.4, 0.1, 0.1)];
        assert_eq!(assemble(&boxes), "你好世界 OK");
        let jp = vec![tb("こんにちは", 0.0, 0.1, 0.2), tb("、世界", 0.3, 0.1, 0.1)];
        assert_eq!(assemble(&jp), "こんにちは、世界");
    }

    #[test]
    fn big_gaps_become_paragraph_breaks() {
        let boxes = vec![tb("Title", 0.0, 0.1, 0.2), tb("Body one", 0.0, 0.25, 0.3), tb("Body two", 0.0, 0.295, 0.3)];
        assert_eq!(assemble(&boxes), "Title\n\nBody one\nBody two");
    }

    #[test]
    fn empty_and_blank_input() {
        assert_eq!(assemble(&[]), "");
        assert_eq!(assemble(&[tb("  ", 0.0, 0.0, 0.1)]), "");
    }

    #[test]
    fn previews_truncate() {
        assert_eq!(preview("short", 20), "short");
        assert_eq!(preview("line one\nline two\nline three", 40), "line one line two …");
        assert_eq!(preview("abcdefghijklmnopqrstuvwxyz", 10), "abcdefghi…");
        assert_eq!(preview("你好世界你好世界", 5), "你好世界…");
    }

    #[test]
    fn cjk_detection() {
        assert!(is_cjk('中') && is_cjk('の') && is_cjk('。') && is_cjk('한'));
        assert!(!is_cjk('a') && !is_cjk('1') && !is_cjk('é'));
        assert_eq!(RECOGNITION_LANGUAGES.len(), 4);
    }
}
