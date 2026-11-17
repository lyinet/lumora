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

pub(crate) fn joiner(left: &str, right: &str) -> &'static str {
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

pub(crate) struct Line<'a> {
    pub(crate) rect: Rect,
    pub(crate) boxes: Vec<&'a TextBox>,
}

/// Group observations into lines in reading order.
pub(crate) fn lines(boxes: &[TextBox]) -> Vec<Line<'_>> {
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

/// One editor's OCR result, so Vision runs once per capture rather than once
/// for auto-redaction, again for Smart Redact and again for every AI task.
///
/// Recognition always reads the editor's unannotated base image, so the
/// cache is keyed by that image (`base` = any stable identity, such as the
/// image's address) and is dropped when the base changes. Edits and the crop
/// do not re-run Vision: they are applied to the cached boxes on every use
/// by [`visible_boxes`], so a cached result can never show text the user
/// has since cropped away or redacted.
#[derive(Debug, Default, Clone)]
pub struct OcrCache {
    entry: Option<(usize, std::sync::Arc<Vec<TextBox>>)>,
}

impl OcrCache {
    /// The boxes recognised on `base` (pixels, y down), if cached.
    pub fn get(&self, base: usize) -> Option<std::sync::Arc<Vec<TextBox>>> {
        match &self.entry {
            Some((b, boxes)) if *b == base => Some(boxes.clone()),
            _ => None,
        }
    }

    /// Remember `boxes` for `base`, replacing whatever was there.
    pub fn store(&mut self, base: usize, boxes: std::sync::Arc<Vec<TextBox>>) {
        self.entry = Some((base, boxes));
    }

    /// Forget the cached result (the base image changed).
    pub fn invalidate(&mut self) {
        self.entry = None;
    }

    pub fn is_cached(&self) -> bool {
        self.entry.is_some()
    }
}

/// Share of a text box that a redaction must cover before the whole box is
/// treated as hidden. Low on purpose: a line with a redacted e-mail address
/// in it is dropped entirely rather than sent to an AI provider.
pub const HIDDEN_COVERAGE: f64 = 0.1;

/// The boxes (pixels of the base, y down) a reader of the edited image would
/// still see: boxes that overlap `crop` (points; `None` = whole image) and
/// are less than [`HIDDEN_COVERAGE`] covered by the `hidden` rects (points,
/// e.g. Redact annotations).
pub fn visible_boxes(boxes: &[TextBox], scale: f64, crop: Option<Rect>, hidden: &[Rect]) -> Vec<TextBox> {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    boxes
        .iter()
        .filter(|b| {
            let r = b.rect.normalized();
            let pt = Rect::new(r.min_x() / s, r.min_y() / s, r.width() / s, r.height() / s);
            if let Some(c) = crop {
                match pt.intersection(&c.normalized()) {
                    Some(i) if i.area() > 0.0 => {}
                    _ => return false,
                }
            }
            let area = pt.area();
            if area <= 0.0 {
                return hidden.iter().all(|h| !h.normalized().contains(pt.origin));
            }
            let covered: f64 = hidden.iter().filter_map(|h| pt.intersection(&h.normalized())).map(|i| i.area()).sum();
            covered / area < HIDDEN_COVERAGE
        })
        .cloned()
        .collect()
}

/// [`visible_boxes`] for an annotation document: its crop, and its Redact
/// shapes as the hidden rects.
pub fn visible_in_document(boxes: &[TextBox], scale: f64, doc: &crate::annotate::Document) -> Vec<TextBox> {
    let hidden: Vec<Rect> = doc
        .items()
        .iter()
        .filter_map(|a| match &a.shape {
            crate::annotate::Shape::Redact { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    visible_boxes(boxes, scale, doc.crop(), &hidden)
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
    #[test]
    fn ocr_cache_is_keyed_by_base_image() {
        let mut c = OcrCache::default();
        assert!(c.get(1).is_none());
        c.store(1, std::sync::Arc::new(vec![TextBox::new("hi", Rect::new(0.0, 0.0, 10.0, 10.0))]));
        assert_eq!(c.get(1).map(|b| b.len()), Some(1));
        // Another base (the image was replaced): miss, never stale data.
        assert!(c.get(2).is_none());
        c.invalidate();
        assert!(c.get(1).is_none());
        assert!(!c.is_cached());
    }

    fn sample() -> Vec<TextBox> {
        // Pixels at scale 2: "top" at points (0,0)-(50,10), "bottom" at
        // points (0,100)-(50,110).
        vec![
            TextBox::new("top", Rect::new(0.0, 0.0, 100.0, 20.0)),
            TextBox::new("bottom", Rect::new(0.0, 200.0, 100.0, 20.0)),
        ]
    }

    fn texts(b: &[TextBox]) -> Vec<&str> {
        b.iter().map(|b| b.text.as_str()).collect()
    }

    #[test]
    fn visible_boxes_follow_the_crop() {
        let b = sample();
        assert_eq!(texts(&visible_boxes(&b, 2.0, None, &[])), ["top", "bottom"]);
        // Crop to the lower half: "top" is cut away.
        let crop = Some(Rect::new(0.0, 60.0, 200.0, 100.0));
        assert_eq!(texts(&visible_boxes(&b, 2.0, crop, &[])), ["bottom"]);
        // A box straddling the crop edge stays.
        let edge = Some(Rect::new(0.0, 5.0, 200.0, 200.0));
        assert_eq!(texts(&visible_boxes(&b, 2.0, edge, &[])), ["top", "bottom"]);
    }

    #[test]
    fn visible_boxes_drop_redacted_text() {
        let b = sample();
        // A redaction over a fifth of "top" hides the whole line.
        let hidden = [Rect::new(40.0, 0.0, 10.0, 10.0)];
        assert_eq!(texts(&visible_boxes(&b, 2.0, None, &hidden)), ["bottom"]);
        // A sliver (under 10%) does not.
        let sliver = [Rect::new(0.0, 0.0, 2.0, 10.0)];
        assert_eq!(texts(&visible_boxes(&b, 2.0, None, &sliver)), ["top", "bottom"]);
    }

    #[test]
    fn visible_in_document_tracks_edits_and_undo() {
        use crate::annotate::{Document, Shape, Style};
        use crate::geometry::Size;
        let b = sample();
        let mut doc = Document::new(Size::new(200.0, 200.0));
        assert_eq!(visible_in_document(&b, 2.0, &doc).len(), 2);
        doc.add(Shape::Redact { rect: Rect::new(0.0, 100.0, 50.0, 10.0), mode: Default::default() }, Style::default());
        assert_eq!(texts(&visible_in_document(&b, 2.0, &doc)), ["top"]);
        assert!(doc.set_crop(Some(Rect::new(0.0, 50.0, 200.0, 150.0))));
        assert!(visible_in_document(&b, 2.0, &doc).is_empty());
        doc.undo();
        doc.undo();
        assert_eq!(visible_in_document(&b, 2.0, &doc).len(), 2);
    }
}
