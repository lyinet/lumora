//! Editor-side AI writing logic: which task a menu item means, what text it
//! works on, how model output is cleaned up, and where the result goes when
//! it is inserted as a text annotation. Pure and unit-tested; the editor
//! and the app only wire it to AppKit and the network.

use super::{AiError, WritingTask};
use crate::annotate::document::AnnotationId;
use crate::annotate::{Document, Shape};
use crate::extract::{
    checklist, layout_listing, parse_markdown_table, reconstruct_code, reconstruct_table, rows_to_csv,
};
use crate::geometry::{Point, Rect, Size};
use crate::i18n::{tr, Language, Msg};
use crate::ocr::TextBox;

/// A writing task as chosen from the editor's AI menu, before its input
/// text is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiTaskKind {
    Caption,
    StepGuide,
    AltText,
    Polish,
    Translate(Language),
    Summarize,
    /// Screenshot → table (Markdown; CSV / TSV on copy).
    ExtractTable,
    /// Screenshot → code with its indentation.
    ExtractCode,
    /// Text → to-do checklist.
    Checklist,
}

impl AiTaskKind {
    /// The menu's top-level entries, in order (Translate expands to one
    /// submenu item per [`Language`]).
    pub const MENU: [AiTaskKind; 8] = [
        AiTaskKind::Caption,
        AiTaskKind::StepGuide,
        AiTaskKind::AltText,
        AiTaskKind::Polish,
        AiTaskKind::Translate(Language::En),
        AiTaskKind::Summarize,
        AiTaskKind::ExtractTable,
        AiTaskKind::ExtractCode,
    ];

    /// The Capture Text panel's actions after Copy, in order (Translate
    /// expands to one menu item per [`Language`]).
    pub const TEXT_ACTIONS: [AiTaskKind; 5] = [
        AiTaskKind::Summarize,
        AiTaskKind::Translate(Language::En),
        AiTaskKind::ExtractTable,
        AiTaskKind::ExtractCode,
        AiTaskKind::Checklist,
    ];

    /// Whether the task works on OCR boxes (positions), not just text.
    pub fn needs_boxes(self) -> bool {
        matches!(self, AiTaskKind::ExtractTable | AiTaskKind::ExtractCode)
    }

    /// Whether there is an on-device result (shown first; AI refines it
    /// when enabled). Tasks without one need AI.
    pub fn has_local(self) -> bool {
        matches!(self, AiTaskKind::ExtractTable | AiTaskKind::ExtractCode | AiTaskKind::Checklist)
    }

    /// Whether the result is a table (offers "Copy as CSV").
    pub fn is_table(self) -> bool {
        self == AiTaskKind::ExtractTable
    }

    /// The Capture Text panel's short button title ("As Table").
    pub fn button_title(self) -> &'static str {
        tr(match self {
            AiTaskKind::Summarize => Msg::AiTaskSummarize,
            AiTaskKind::Translate(_) => Msg::AiTaskTranslate,
            AiTaskKind::ExtractTable => Msg::TextAsTable,
            AiTaskKind::ExtractCode => Msg::TextAsCode,
            AiTaskKind::Checklist => Msg::AiTaskChecklist,
            AiTaskKind::Caption => Msg::AiTaskCaption,
            AiTaskKind::StepGuide => Msg::AiTaskStepGuide,
            AiTaskKind::AltText => Msg::AiTaskAltText,
            AiTaskKind::Polish => Msg::AiTaskPolish,
        })
    }

    /// Whether the task works on a piece of text: the selected text
    /// annotation if there is one, otherwise the recognised text.
    pub fn uses_selection(self) -> bool {
        matches!(self, AiTaskKind::Polish | AiTaskKind::Translate(_))
    }

    /// Whether the result can replace the source text annotation.
    pub fn replaces_source(self) -> bool {
        self.uses_selection()
    }

    /// Whether the task needs on-device text recognition. Polish and
    /// Translate of a selected annotation don't; everything else reads the
    /// screenshot's text.
    pub fn needs_ocr(self, has_source: bool) -> bool {
        !(self.uses_selection() && has_source)
    }

    /// The concrete task. `text` is the source annotation's text, or the
    /// recognised text when there is no source (Polish / Translate);
    /// Summarize with an empty `text` summarises the recognised text.
    pub fn task(self, text: impl Into<String>) -> WritingTask {
        let text = text.into();
        match self {
            AiTaskKind::Caption => WritingTask::Caption,
            AiTaskKind::StepGuide => WritingTask::StepGuide,
            AiTaskKind::AltText => WritingTask::AltText,
            AiTaskKind::Polish => WritingTask::Polish(text),
            AiTaskKind::Translate(to) => WritingTask::Translate { text, to },
            AiTaskKind::Summarize => WritingTask::Summarize(text),
            AiTaskKind::ExtractTable => WritingTask::ExtractTable(text),
            AiTaskKind::ExtractCode => WritingTask::ExtractCode(text),
            AiTaskKind::Checklist => WritingTask::Checklist(text),
        }
    }

    /// Whether the screenshot image helps (sent only with `send_image`).
    pub fn uses_image(self) -> bool {
        self.task(String::new()).uses_image()
    }

    /// Menu / result-sheet title, e.g. "Translate → English".
    pub fn title(self) -> String {
        match self {
            AiTaskKind::Translate(to) => format!("{} → {}", tr(Msg::AiTaskTranslate), language_name(to)),
            k => k.task(String::new()).label().to_string(),
        }
    }
}

/// The on-device result of `kind` for OCR runs `boxes` (assembled text
/// `text`): Markdown table, code, or checklist. `None` for AI-only tasks
/// and when nothing was recognised.
pub fn local_result(kind: AiTaskKind, boxes: &[TextBox], text: &str) -> Option<String> {
    let out = match kind {
        AiTaskKind::ExtractTable => reconstruct_table(boxes).to_markdown(),
        AiTaskKind::ExtractCode => reconstruct_code(boxes),
        AiTaskKind::Checklist => checklist(text),
        _ => return None,
    };
    (!out.trim().is_empty()).then_some(out)
}

/// The text an AI request for `kind` works on: the positioned listing for
/// tables, the locally indented code for code, the plain text otherwise.
pub fn ai_input(kind: AiTaskKind, boxes: &[TextBox], text: &str) -> String {
    match kind {
        AiTaskKind::ExtractTable if !boxes.is_empty() => layout_listing(boxes),
        AiTaskKind::ExtractCode if !boxes.is_empty() => reconstruct_code(boxes),
        _ => text.to_string(),
    }
}

/// CSV for a table result: parsed from the Markdown table shown (local or
/// AI), else every line as one cell.
pub fn table_csv(markdown: &str) -> String {
    match parse_markdown_table(markdown) {
        Some(rows) => rows_to_csv(&rows),
        None => rows_to_csv(
            &markdown.lines().filter(|l| !l.trim().is_empty()).map(|l| vec![l.trim().to_string()]).collect::<Vec<_>>(),
        ),
    }
}

/// A language's name in that language (menu items for Translate).
pub fn language_name(l: Language) -> &'static str {
    match l {
        Language::En => "English",
        Language::ZhHans => "简体中文",
    }
}

/// The text of annotation `id` if it is a non-empty text annotation.
pub fn text_of(doc: &Document, id: Option<AnnotationId>) -> Option<(AnnotationId, String)> {
    let a = doc.get(id?)?;
    match &a.shape {
        Shape::Text { text, .. } if !text.trim().is_empty() => Some((a.id, text.clone())),
        _ => None,
    }
}

/// Errors whose fix is in Settings → AI (the sheet offers a button).
pub fn error_opens_settings(e: &AiError) -> bool {
    matches!(e, AiError::MissingKey | AiError::BadKey(_) | AiError::BadConfig)
}

/// Tidy a model reply for display and insertion: normalise line endings,
/// drop a wrapping Markdown code fence or a pair of quotes around the whole
/// reply, strip trailing spaces and collapse runs of blank lines.
pub fn clean_output(raw: &str) -> String {
    let mut s = raw.replace("\r\n", "\n").replace('\r', "\n");
    s = s.trim().to_string();
    // ```lang\n … \n```
    if s.starts_with("```") && s.ends_with("```") && s.len() >= 6 {
        let inner = &s[3..s.len() - 3];
        let inner = match inner.find('\n') {
            // The opening line may name a language ("```markdown").
            Some(nl) if !inner[..nl].trim().contains(' ') => &inner[nl + 1..],
            Some(_) => inner,
            None => inner,
        };
        s = inner.trim().to_string();
    }
    for (open, close) in [("\"", "\""), ("“", "”"), ("「", "」"), ("『", "』"), ("'", "'")] {
        if s.len() > open.len() + close.len() && s.starts_with(open) && s.ends_with(close) {
            let inner = &s[open.len()..s.len() - close.len()];
            // Only a single quoted span: "a" and "b" keeps its quotes.
            if !inner.contains(open) && !inner.contains(close) {
                s = inner.trim().to_string();
                break;
            }
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

fn char_width(c: char) -> usize {
    if crate::ocr::is_cjk(c) {
        2
    } else {
        1
    }
}

/// Hard-wrap `text` for a text annotation at `columns` (CJK characters
/// count double). Breaks at spaces where possible, anywhere in CJK runs,
/// and keeps existing line breaks.
pub fn wrap(text: &str, columns: usize) -> String {
    let columns = columns.max(8);
    let mut out: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        let mut width = 0;
        // Tokens: a word plus its trailing space, or a single CJK char.
        let mut tokens: Vec<String> = Vec::new();
        let mut cur = String::new();
        for c in para.chars() {
            if char_width(c) == 2 {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
                tokens.push(c.to_string());
            } else {
                cur.push(c);
                if c == ' ' {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
        for tok in tokens {
            let w: usize = tok.chars().map(char_width).sum();
            if width + w.saturating_sub(usize::from(tok.ends_with(' '))) > columns && width > 0 {
                out.push(line.trim_end().to_string());
                line.clear();
                width = 0;
                if tok == " " {
                    continue;
                }
            }
            if w > columns {
                // A single over-long word: split it.
                for c in tok.chars() {
                    if width + char_width(c) > columns && width > 0 {
                        out.push(std::mem::take(&mut line));
                        width = 0;
                    }
                    line.push(c);
                    width += char_width(c);
                }
                continue;
            }
            line.push_str(&tok);
            width += w;
        }
        out.push(line.trim_end().to_string());
    }
    out.join("\n")
}

/// What "Insert as text" does with a finished result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InsertPlan {
    /// Replace the text of this annotation (one undo step).
    Replace(AnnotationId),
    /// Add a new text annotation with its origin here.
    Insert(Point),
}

/// Margin from the visible edge for inserted text (points).
pub const INSERT_MARGIN: f64 = 16.0;

/// Decide where a result of `kind` goes. A Polish / Translate result
/// replaces its `source` annotation while that still exists as text;
/// otherwise a new annotation goes near the top-left of the visible area,
/// moved down past existing text annotations that would overlap it, and
/// kept inside the visible rect when it fits.
pub fn plan_insert(kind: AiTaskKind, source: Option<AnnotationId>, doc: &Document, size: Size) -> InsertPlan {
    if kind.replaces_source() {
        if let Some((id, _)) = text_of(doc, source) {
            return InsertPlan::Replace(id);
        }
    }
    InsertPlan::Insert(insert_point(doc, size))
}

fn insert_point(doc: &Document, size: Size) -> Point {
    let vis = doc.visible_rect();
    let x = vis.min_x() + INSERT_MARGIN.min(vis.width() / 4.0);
    let mut y = vis.min_y() + INSERT_MARGIN.min(vis.height() / 4.0);
    let occupied: Vec<Rect> = doc.items().iter().filter_map(|a| a.shape.text_rect(&a.style)).collect();
    // A few passes: each overlap pushes the candidate below that box.
    for _ in 0..occupied.len() + 1 {
        let cand = Rect::new(x, y, size.width, size.height);
        let bottom = occupied.iter().filter(|r| intersects(r, &cand)).map(|r| r.max_y()).reduce(f64::max);
        match bottom {
            Some(bottom) => y = bottom + 8.0,
            None => break,
        }
    }
    // Keep it on the visible area when it fits.
    let max_y = vis.max_y() - size.height - INSERT_MARGIN.min(vis.height() / 4.0);
    if y > max_y {
        y = max_y.max(vis.min_y());
    }
    Point::new(x, y)
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.min_x() < b.max_x() && b.min_x() < a.max_x() && a.min_y() < b.max_y() && b.min_y() < a.max_y()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::style::Style;

    fn text(s: &str, at: Point) -> Shape {
        Shape::Text { origin: at, text: s.into(), size: Size::new(100.0, 20.0), background: false }
    }

    #[test]
    fn kinds_map_to_tasks() {
        assert_eq!(AiTaskKind::Caption.task("x"), WritingTask::Caption);
        assert_eq!(AiTaskKind::Polish.task("hi"), WritingTask::Polish("hi".into()));
        assert_eq!(
            AiTaskKind::Translate(Language::ZhHans).task("hi"),
            WritingTask::Translate { text: "hi".into(), to: Language::ZhHans }
        );
        assert_eq!(AiTaskKind::Summarize.task(""), WritingTask::Summarize(String::new()));
        assert!(AiTaskKind::Caption.uses_image() && !AiTaskKind::Polish.uses_image());
        assert!(AiTaskKind::Polish.uses_selection() && !AiTaskKind::Summarize.uses_selection());
        assert!(!AiTaskKind::Polish.needs_ocr(true));
        assert!(AiTaskKind::Polish.needs_ocr(false));
        assert!(AiTaskKind::Caption.needs_ocr(true));
        assert!(AiTaskKind::Translate(Language::En).title().contains("English"));
    }

    #[test]
    fn extraction_kinds() {
        assert_eq!(AiTaskKind::ExtractTable.task("rows"), WritingTask::ExtractTable("rows".into()));
        assert_eq!(AiTaskKind::Checklist.task("a"), WritingTask::Checklist("a".into()));
        assert!(AiTaskKind::ExtractTable.needs_boxes() && !AiTaskKind::Checklist.needs_boxes());
        assert!(AiTaskKind::ExtractCode.has_local() && !AiTaskKind::Summarize.has_local());
        assert!(AiTaskKind::ExtractTable.is_table() && !AiTaskKind::ExtractCode.is_table());
        assert!(AiTaskKind::ExtractTable.needs_ocr(true));
        assert!(!AiTaskKind::ExtractCode.uses_selection() && !AiTaskKind::ExtractCode.uses_image());
        assert!(AiTaskKind::MENU.contains(&AiTaskKind::ExtractTable));
        assert!(AiTaskKind::MENU.contains(&AiTaskKind::ExtractCode));
        for k in AiTaskKind::TEXT_ACTIONS {
            assert!(!k.button_title().is_empty());
        }
        assert_ne!(AiTaskKind::ExtractTable.button_title(), AiTaskKind::ExtractTable.title());
    }

    #[test]
    fn local_results_and_inputs() {
        let b = |t: &str, x: f64, y: f64| TextBox::new(t, Rect::new(x, y, 10.0 * t.len() as f64, 20.0));
        let boxes = vec![b("Name", 0.0, 0.0), b("Age", 200.0, 0.0), b("Ann", 0.0, 30.0), b("30", 200.0, 30.0)];
        let text = crate::ocr::assemble(&boxes);
        let md = local_result(AiTaskKind::ExtractTable, &boxes, &text).unwrap();
        assert_eq!(md, "| Name | Age |\n| --- | --- |\n| Ann | 30 |");
        assert_eq!(table_csv(&md), "Name,Age\nAnn,30\n");
        assert_eq!(table_csv("just\nlines"), "just\nlines\n");
        assert_eq!(local_result(AiTaskKind::Checklist, &boxes, "a\nb").unwrap(), "- [ ] a\n- [ ] b");
        assert!(local_result(AiTaskKind::ExtractCode, &boxes, &text).is_some());
        assert_eq!(local_result(AiTaskKind::Summarize, &boxes, &text), None);
        assert_eq!(local_result(AiTaskKind::ExtractTable, &[], ""), None);
        assert!(ai_input(AiTaskKind::ExtractTable, &boxes, &text).starts_with("row 1:"));
        assert_eq!(ai_input(AiTaskKind::Summarize, &boxes, "t"), "t");
        assert_eq!(ai_input(AiTaskKind::ExtractTable, &[], "t"), "t");
    }

    #[test]
    fn settings_errors() {
        assert!(error_opens_settings(&AiError::MissingKey));
        assert!(error_opens_settings(&AiError::BadKey(401)));
        assert!(error_opens_settings(&AiError::BadConfig));
        assert!(!error_opens_settings(&AiError::Timeout));
        assert!(!error_opens_settings(&AiError::Cancelled));
    }

    #[test]
    fn cleans_fences_quotes_and_blank_lines() {
        assert_eq!(clean_output("```\nhello\n```"), "hello");
        assert_eq!(clean_output("```markdown\n# Title\n\nbody\n```"), "# Title\n\nbody");
        assert_eq!(clean_output("  \"Polished text.\"  "), "Polished text.");
        assert_eq!(clean_output("“你好，世界”"), "你好，世界");
        assert_eq!(clean_output("「こんにちは」"), "こんにちは");
        assert_eq!(clean_output("\"a\" and \"b\""), "\"a\" and \"b\"");
        assert_eq!(clean_output("a\r\n\r\n\r\n\r\nb  \n"), "a\n\nb");
        assert_eq!(clean_output("plain"), "plain");
        assert_eq!(clean_output(""), "");
        assert_eq!(clean_output("```"), "```");
    }

    #[test]
    fn wraps_words_and_cjk() {
        assert_eq!(wrap("the quick brown fox jumps", 10), "the quick\nbrown fox\njumps");
        assert_eq!(wrap("一二三四五六七八九十", 8), "一二三四\n五六七八\n九十");
        assert_eq!(wrap("short\n\nkeep", 40), "short\n\nkeep");
        assert_eq!(wrap("abcdefghijklmnop", 8), "abcdefgh\nijklmnop");
        assert_eq!(wrap("点击 Save 按钮", 40), "点击 Save 按钮");
    }

    #[test]
    fn source_text_lookup() {
        let mut doc = Document::new(Size::new(400.0, 300.0));
        let t = doc.add(text("Hello", Point::new(10.0, 10.0)), Style::default());
        let blank = doc.add(text("  ", Point::new(10.0, 50.0)), Style::default());
        let r = doc.add(Shape::Step { center: Point::new(50.0, 50.0) }, Style::default());
        assert_eq!(text_of(&doc, Some(t)), Some((t, "Hello".into())));
        assert_eq!(text_of(&doc, Some(blank)), None);
        assert_eq!(text_of(&doc, Some(r)), None);
        assert_eq!(text_of(&doc, None), None);
    }

    #[test]
    fn replace_only_for_existing_source_text() {
        let mut doc = Document::new(Size::new(400.0, 300.0));
        let t = doc.add(text("Hello", Point::new(200.0, 200.0)), Style::default());
        let s = Size::new(80.0, 20.0);
        assert_eq!(plan_insert(AiTaskKind::Polish, Some(t), &doc, s), InsertPlan::Replace(t));
        assert_eq!(plan_insert(AiTaskKind::Translate(Language::En), Some(t), &doc, s), InsertPlan::Replace(t));
        // Caption never replaces.
        assert!(matches!(plan_insert(AiTaskKind::Caption, Some(t), &doc, s), InsertPlan::Insert(_)));
        // Source deleted meanwhile → insert.
        doc.remove(t);
        assert!(matches!(plan_insert(AiTaskKind::Polish, Some(t), &doc, s), InsertPlan::Insert(_)));
    }

    #[test]
    fn insert_point_top_left_and_avoids_text() {
        let mut doc = Document::new(Size::new(400.0, 300.0));
        let s = Size::new(120.0, 40.0);
        assert_eq!(plan_insert(AiTaskKind::Caption, None, &doc, s), InsertPlan::Insert(Point::new(16.0, 16.0)));
        doc.add(text("Title", Point::new(10.0, 10.0)), Style::default());
        match plan_insert(AiTaskKind::Caption, None, &doc, s) {
            InsertPlan::Insert(p) => {
                assert_eq!(p.x, 16.0);
                assert!(p.y >= 30.0, "{p:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn insert_point_stays_visible() {
        let mut doc = Document::new(Size::new(400.0, 100.0));
        for i in 0..4 {
            doc.add(text("x", Point::new(10.0, 10.0 + 22.0 * f64::from(i))), Style::default());
        }
        let s = Size::new(120.0, 40.0);
        let InsertPlan::Insert(p) = plan_insert(AiTaskKind::Caption, None, &doc, s) else { panic!() };
        assert!(p.y + s.height <= 100.0, "{p:?}");
        // Respects the crop.
        let mut doc = Document::new(Size::new(400.0, 300.0));
        doc.set_crop(Some(Rect::new(100.0, 100.0, 200.0, 150.0)));
        let InsertPlan::Insert(p) = plan_insert(AiTaskKind::Caption, None, &doc, s) else { panic!() };
        assert_eq!(p, Point::new(116.0, 116.0));
    }
}
