//! Local (no-AI) structure recovery from OCR boxes: a table rebuilt by
//! grouping runs into rows (y) and clustering their x ranges into columns,
//! code with its indentation restored from each line's x offset, and a
//! to-do checklist from plain lines. These are the instant results of the
//! Capture Text panel's "As Table" / "As Code" / "Checklist" actions; AI,
//! when enabled, refines them.

use crate::geometry::Rect;
use crate::ocr::{is_cjk, joiner, lines, TextBox};

/// A rebuilt table: `rows[r][c]`, every row padded to the same width.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub rows: Vec<Vec<String>>,
}

impl Table {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn columns(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }

    /// GitHub-flavoured Markdown; the first row is the header.
    pub fn to_markdown(&self) -> String {
        let Some(first) = self.rows.first() else { return String::new() };
        let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
        let row = |r: &[String]| format!("| {} |", r.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | "));
        let mut out = vec![row(first), format!("|{}", " --- |".repeat(first.len()))];
        out.extend(self.rows[1..].iter().map(|r| row(r)));
        out.join("\n")
    }

    /// RFC 4180 CSV (CRLF-free: rows end with `\n`).
    pub fn to_csv(&self) -> String {
        rows_to_csv(&self.rows)
    }

    /// Tab-separated values (tabs and newlines inside cells become spaces).
    pub fn to_tsv(&self) -> String {
        self.rows
            .iter()
            .map(|r| r.iter().map(|c| c.replace(['\t', '\n', '\r'], " ")).collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// CSV for arbitrary rows: fields with `,` `"` or line breaks are quoted.
pub fn rows_to_csv(rows: &[Vec<String>]) -> String {
    let field = |s: &str| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::new();
    for r in rows {
        out.push_str(&r.iter().map(|c| field(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Column spans: the union of the x ranges of runs in rows that have at
/// least two runs (single-run rows are titles or wrapped notes and would
/// glue every column together). Ranges closer than `tol` merge.
fn column_spans(rows: &[Vec<&TextBox>], tol: f64) -> Vec<(f64, f64)> {
    let mut ranges: Vec<(f64, f64)> =
        rows.iter().filter(|r| r.len() >= 2).flatten().map(|b| (b.rect.min_x(), b.rect.max_x())).collect();
    if ranges.is_empty() {
        return vec![(f64::MIN, f64::MAX)];
    }
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for (a, b) in ranges {
        match spans.last_mut() {
            Some(last) if a <= last.1 + tol => last.1 = last.1.max(b),
            _ => spans.push((a, b)),
        }
    }
    spans
}

fn column_of(spans: &[(f64, f64)], r: &Rect) -> usize {
    let cx = r.mid().x;
    if let Some(i) = spans.iter().position(|(a, b)| cx >= *a && cx <= *b) {
        return i;
    }
    // Nearest span edge.
    spans
        .iter()
        .enumerate()
        .map(|(i, (a, b))| (i, if cx < *a { a - cx } else { cx - b }))
        .min_by(|x, y| x.1.total_cmp(&y.1))
        .map_or(0, |(i, _)| i)
}

/// Rebuild a table from OCR runs: rows by line grouping, columns by
/// clustering x ranges. Runs falling in the same cell are joined.
pub fn reconstruct_table(boxes: &[TextBox]) -> Table {
    let ls = lines(boxes);
    if ls.is_empty() {
        return Table::default();
    }
    let tol = median(ls.iter().map(|l| l.rect.height()).collect()) * 0.6;
    let rows: Vec<Vec<&TextBox>> = ls.into_iter().map(|l| l.boxes).collect();
    let spans = column_spans(&rows, tol);
    let n = spans.len();
    let rows = rows
        .iter()
        .map(|r| {
            let mut cells = vec![String::new(); n];
            for b in r {
                let c = &mut cells[column_of(&spans, &b.rect)];
                let t = b.text.trim();
                if !c.is_empty() {
                    c.push_str(joiner(c, t));
                }
                c.push_str(t);
            }
            cells
        })
        .collect();
    Table { rows }
}

fn char_units(s: &str) -> f64 {
    s.chars().map(|c| if is_cjk(c) { 2.0 } else { 1.0 }).sum()
}

/// Rebuild code: lines in reading order, leading spaces from each line's x
/// offset relative to the leftmost line (in estimated character widths,
/// snapped to the indent unit), gaps inside a line kept as spaces, and
/// blank lines where the vertical gap is large.
pub fn reconstruct_code(boxes: &[TextBox]) -> String {
    let ls = lines(boxes);
    if ls.is_empty() {
        return String::new();
    }
    // Character width: median of box width / characters (monospace-ish).
    let char_w = median(
        boxes
            .iter()
            .filter(|b| char_units(b.text.trim()) >= 2.0)
            .map(|b| b.rect.width() / char_units(b.text.trim()))
            .collect(),
    );
    let char_w = if char_w > 0.0 { char_w } else { median(ls.iter().map(|l| l.rect.height()).collect()) * 0.6 };
    let line_h = median(ls.iter().map(|l| l.rect.height()).collect());
    let min_x = ls.iter().map(|l| l.rect.min_x()).fold(f64::MAX, f64::min);
    let raw: Vec<f64> = ls.iter().map(|l| ((l.rect.min_x() - min_x) / char_w).max(0.0)).collect();
    // Indent unit: the smallest clear indent, clamped to 2…8 (default 4).
    let unit = raw.iter().copied().filter(|r| *r >= 1.0).fold(f64::MAX, f64::min);
    let unit = if unit == f64::MAX { 4.0 } else { unit.round().clamp(2.0, 8.0) };
    let mut out = String::new();
    let mut prev_bottom: Option<f64> = None;
    for (l, r) in ls.iter().zip(raw) {
        if let Some(pb) = prev_bottom {
            out.push('\n');
            if l.rect.min_y() - pb > line_h * 1.2 {
                out.push('\n');
            }
        }
        let indent = ((r / unit).round() * unit) as usize;
        out.push_str(&" ".repeat(indent));
        let mut right: Option<f64> = None;
        for b in &l.boxes {
            if let Some(x) = right {
                let gap = ((b.rect.min_x() - x) / char_w).round().max(1.0) as usize;
                out.push_str(&" ".repeat(gap.min(40)));
            }
            out.push_str(b.text.trim());
            right = Some(b.rect.max_x());
        }
        prev_bottom = Some(l.rect.max_y());
    }
    out
}

/// One line per item as `- [ ] item`. Existing bullets, numbers and check
/// boxes are replaced; ticked items (☑ ✓ ✔ [x]) stay ticked.
pub fn checklist(text: &str) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut s = line.trim();
        if s.is_empty() {
            continue;
        }
        let mut done = false;
        for (p, d) in
            [("[x]", true), ("[X]", true), ("[ ]", false), ("☑", true), ("✅", true), ("✓", true), ("✔", true)]
        {
            if let Some(rest) = s.strip_prefix(p) {
                s = rest.trim_start();
                done = d;
                break;
            }
        }
        // A list marker after a check box ("- [ ]") or before ("☐").
        for m in ["- ", "* ", "• ", "· ", "◦ ", "▪ ", "☐ ", "□ ", "-", "*", "•", "·", "☐", "□"] {
            if let Some(rest) = s.strip_prefix(m) {
                s = rest.trim_start();
                break;
            }
        }
        for (p, d) in [("[x]", true), ("[X]", true), ("[ ]", false)] {
            if let Some(rest) = s.strip_prefix(p) {
                s = rest.trim_start();
                done |= d;
                break;
            }
        }
        // "1." "2)" "(3)" "４、"
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && digits.len() <= 3 {
            let rest = &s[digits.len()..];
            for sep in [". ", ") ", "、", "．", ".", ")"] {
                if let Some(r) = rest.strip_prefix(sep) {
                    if !r.trim().is_empty() {
                        s = r.trim_start();
                    }
                    break;
                }
            }
        }
        if s.is_empty() {
            continue;
        }
        out.push(format!("- [{}] {s}", if done { "x" } else { " " }));
    }
    out.join("\n")
}

/// Rows of the first Markdown table in `md` (the separator row dropped,
/// `\|` unescaped). `None` when there is no table.
pub fn parse_markdown_table(md: &str) -> Option<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    for line in md.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            if rows.is_empty() {
                continue;
            }
            break;
        }
        let inner = t.trim_start_matches('|');
        let inner = inner.strip_suffix('|').filter(|s| !s.ends_with('\\')).unwrap_or(inner);
        let mut cells = Vec::new();
        let mut cur = String::new();
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&'|') => {
                    cur.push('|');
                    chars.next();
                }
                '|' => cells.push(std::mem::take(&mut cur).trim().to_string()),
                c => cur.push(c),
            }
        }
        cells.push(cur.trim().to_string());
        let separator = cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
        if !separator {
            rows.push(cells);
        }
    }
    (!rows.is_empty()).then_some(rows)
}

/// The OCR runs as rows of positioned text, for the AI table prompt:
/// `row 1: [x=12] Name | [x=200] Age`.
pub fn layout_listing(boxes: &[TextBox]) -> String {
    let ls = lines(boxes);
    let min_x = ls.iter().map(|l| l.rect.min_x()).fold(f64::MAX, f64::min);
    ls.iter()
        .enumerate()
        .map(|(i, l)| {
            let runs: Vec<String> = l
                .boxes
                .iter()
                .map(|b| format!("[x={}] {}", (b.rect.min_x() - min_x).round() as i64, b.text.trim()))
                .collect();
            format!("row {}: {}", i + 1, runs.join(" | "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tb(t: &str, x: f64, y: f64, w: f64) -> TextBox {
        TextBox::new(t, Rect::new(x, y, w, 20.0))
    }

    fn grid() -> Vec<TextBox> {
        vec![
            tb("Name", 10.0, 10.0, 60.0),
            tb("Age", 200.0, 10.0, 40.0),
            tb("City", 320.0, 10.0, 50.0),
            tb("Alice", 10.0, 40.0, 70.0),
            tb("30", 205.0, 40.0, 20.0),
            tb("Paris", 322.0, 40.0, 60.0),
            // Missing middle cell.
            tb("Bob", 12.0, 70.0, 40.0),
            tb("New York", 318.0, 70.0, 100.0),
        ]
    }

    #[test]
    fn table_rows_and_columns() {
        let t = reconstruct_table(&grid());
        assert_eq!(t.columns(), 3);
        assert_eq!(
            t.rows,
            vec![
                vec!["Name".to_string(), "Age".into(), "City".into()],
                vec!["Alice".into(), "30".into(), "Paris".into()],
                vec!["Bob".into(), String::new(), "New York".into()],
            ]
        );
        assert_eq!(
            t.to_markdown(),
            "| Name | Age | City |\n| --- | --- | --- |\n| Alice | 30 | Paris |\n| Bob |  | New York |"
        );
        assert_eq!(t.to_tsv(), "Name\tAge\tCity\nAlice\t30\tParis\nBob\t\tNew York");
    }

    #[test]
    fn table_title_row_does_not_merge_columns() {
        let mut b = grid();
        b.push(tb("Quarterly staff list for all offices", 0.0, -30.0, 400.0));
        let t = reconstruct_table(&b);
        assert_eq!(t.columns(), 3);
        assert_eq!(t.rows.len(), 4);
        assert!(t.rows[0][0].starts_with("Quarterly") || t.rows[0][1].starts_with("Quarterly"));
    }

    #[test]
    fn table_single_column_and_empty() {
        assert!(reconstruct_table(&[]).is_empty());
        assert_eq!(reconstruct_table(&[]).to_markdown(), "");
        let t = reconstruct_table(&[tb("one", 0.0, 0.0, 30.0), tb("two", 0.0, 30.0, 30.0)]);
        assert_eq!(t.rows, vec![vec!["one".to_string()], vec!["two".to_string()]]);
    }

    #[test]
    fn csv_quotes_and_markdown_escapes() {
        let t = Table { rows: vec![vec!["a,b".into(), "say \"hi\"".into()], vec!["x|y".into(), "z".into()]] };
        assert_eq!(t.to_csv(), "\"a,b\",\"say \"\"hi\"\"\"\nx|y,z\n");
        let md = t.to_markdown();
        assert!(md.contains("x\\|y"));
        assert_eq!(parse_markdown_table(&md).unwrap(), t.rows);
    }

    #[test]
    fn markdown_table_parsing() {
        let md = "Here you go:\n\n| A | B |\n|:--|--:|\n| 1 | 2 |\n| 3 |  |\n\nDone.";
        assert_eq!(
            parse_markdown_table(md).unwrap(),
            vec![vec!["A".to_string(), "B".into()], vec!["1".into(), "2".into()], vec!["3".into(), String::new()]]
        );
        assert_eq!(parse_markdown_table("no table here"), None);
        assert_eq!(rows_to_csv(&parse_markdown_table(md).unwrap()), "A,B\n1,2\n3,\n");
    }

    #[test]
    fn code_indentation_restored() {
        // 10 px per character.
        let b = vec![
            tb("fn main() {", 100.0, 0.0, 110.0),
            tb("let x = 1;", 140.0, 24.0, 100.0),
            tb("if x > 0 {", 140.0, 48.0, 100.0),
            tb("go();", 181.0, 72.0, 50.0),
            tb("}", 139.0, 96.0, 10.0),
            tb("}", 100.0, 160.0, 10.0),
        ];
        assert_eq!(reconstruct_code(&b), "fn main() {\n    let x = 1;\n    if x > 0 {\n        go();\n    }\n\n}");
    }

    #[test]
    fn code_gaps_inside_a_line() {
        let b = vec![tb("x", 0.0, 0.0, 10.0), tb("= 1  # one", 30.0, 0.0, 100.0)];
        assert_eq!(reconstruct_code(&b), "x  = 1  # one");
        assert_eq!(reconstruct_code(&[]), "");
    }

    #[test]
    fn two_space_indent_unit() {
        let b = vec![tb("a:", 0.0, 0.0, 20.0), tb("b: 1", 20.0, 24.0, 40.0), tb("c: 2", 40.0, 48.0, 40.0)];
        assert_eq!(reconstruct_code(&b), "a:\n  b: 1\n    c: 2");
    }

    #[test]
    fn checklist_from_lines() {
        let text = "Buy milk\n- Call Ann\n2. Send report\n☑ Book flight\n\n• 写周报\n[x] done already\n- [ ] keep";
        assert_eq!(
            checklist(text),
            "- [ ] Buy milk\n- [ ] Call Ann\n- [ ] Send report\n- [x] Book flight\n- [ ] 写周报\n- [x] done already\n- [ ] keep"
        );
        assert_eq!(checklist("  \n "), "");
        assert_eq!(checklist("2026 plans"), "- [ ] 2026 plans");
    }

    #[test]
    fn listing_for_prompts() {
        let l = layout_listing(&grid());
        assert_eq!(l.lines().count(), 3);
        assert!(l.starts_with("row 1: [x=0] Name | [x=190] Age | [x=310] City"));
    }
}
