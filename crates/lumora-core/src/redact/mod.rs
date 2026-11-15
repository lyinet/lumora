//! Smart redaction (智能打码): find personal and secret data in a capture.
//!
//! Local-first. Text detectors run over the on-device OCR result (boxes
//! from Vision): emails, phone numbers, Chinese ID cards, bank cards, API
//! keys and tokens, IPv4 addresses and secrets in URL queries. Face boxes
//! come from Vision on macOS. An optional AI pass (off by default) sends
//! only the recognised text lines — never the image — to the configured
//! model. Every hit becomes a rectangle for a redaction annotation.

pub mod ai;
pub mod detect;
pub mod layout;

use serde::{Deserialize, Serialize};

use crate::annotate::RedactMode;
use crate::geometry::Rect;
use crate::ocr::TextBox;

pub use detect::{detect, Hit};
pub use layout::{ocr_lines, OcrLine};

/// What a redaction hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Email,
    Phone,
    IdCard,
    BankCard,
    Secret,
    IpAddress,
    UrlToken,
    Face,
    /// Flagged by the AI pass without a known category.
    Other,
}

impl Category {
    /// The categories with a Settings checkbox (faces have their own).
    pub const TEXT: [Category; 7] = [
        Category::Email,
        Category::Phone,
        Category::IdCard,
        Category::BankCard,
        Category::Secret,
        Category::IpAddress,
        Category::UrlToken,
    ];
}

/// `[redact]` section of `settings.toml`. Every field has a default, so
/// files from before v0.5 load unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RedactSettings {
    pub email: bool,
    pub phone: bool,
    pub id_card: bool,
    pub bank_card: bool,
    pub secret: bool,
    pub ip_address: bool,
    pub url_token: bool,
    /// Detect faces (Vision, on device).
    pub faces: bool,
    /// How detected areas are hidden.
    pub mode: RedactMode,
    /// Also ask the AI model which text lines are sensitive (text only).
    /// Needs AI writing to be enabled; off by default.
    pub ai_check: bool,
    /// Redact every new screenshot before it is copied or saved (local
    /// detectors only). Off by default.
    pub auto_redact: bool,
}

impl Default for RedactSettings {
    fn default() -> Self {
        RedactSettings {
            email: true,
            phone: true,
            id_card: true,
            bank_card: true,
            secret: true,
            ip_address: true,
            url_token: true,
            faces: true,
            mode: RedactMode::Pixelate,
            ai_check: false,
            auto_redact: false,
        }
    }
}

impl RedactSettings {
    pub fn enabled(&self, c: Category) -> bool {
        match c {
            Category::Email => self.email,
            Category::Phone => self.phone,
            Category::IdCard => self.id_card,
            Category::BankCard => self.bank_card,
            Category::Secret => self.secret,
            Category::IpAddress => self.ip_address,
            Category::UrlToken => self.url_token,
            Category::Face => self.faces,
            Category::Other => true,
        }
    }

    pub fn set(&mut self, c: Category, on: bool) {
        let slot = match c {
            Category::Email => &mut self.email,
            Category::Phone => &mut self.phone,
            Category::IdCard => &mut self.id_card,
            Category::BankCard => &mut self.bank_card,
            Category::Secret => &mut self.secret,
            Category::IpAddress => &mut self.ip_address,
            Category::UrlToken => &mut self.url_token,
            Category::Face => &mut self.faces,
            Category::Other => return,
        };
        *slot = on;
    }
}

/// One thing to hide. `rects` are in the OCR boxes' space (one per box the
/// match touched).
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub category: Category,
    pub rects: Vec<Rect>,
}

/// Padding around text matches, as a fraction of the line height.
pub const TEXT_PAD: f64 = 0.15;

/// Run the enabled text detectors over OCR `boxes`.
pub fn find_in_text(boxes: &[TextBox], settings: &RedactSettings) -> Vec<Finding> {
    find_in_lines(&ocr_lines(boxes), settings)
}

/// [`find_in_text`] over lines already grouped with [`ocr_lines`].
pub fn find_in_lines(lines: &[OcrLine], settings: &RedactSettings) -> Vec<Finding> {
    lines
        .iter()
        .flat_map(|line| {
            detect(&line.text, |c| settings.enabled(c))
                .into_iter()
                .map(|h| Finding { category: h.category, rects: layout::hit_rects(line, h.range, TEXT_PAD) })
                .filter(|f| !f.rects.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Whole-line findings for AI-flagged `(line, category)` pairs, skipping
/// lines a local detector already covered completely.
pub fn findings_for_lines(lines: &[OcrLine], flagged: &[(usize, Category)], existing: &[Finding]) -> Vec<Finding> {
    flagged
        .iter()
        .filter_map(|&(i, category)| {
            let line = lines.get(i)?;
            let p = line.rect.height() * TEXT_PAD;
            let rect = line.rect.inset(-p, -p * 0.8);
            let covered = existing.iter().flat_map(|f| f.rects.iter()).any(|r| r.contains_rect(&rect));
            (!covered).then(|| Finding { category, rects: vec![rect] })
        })
        .collect()
}

/// Face boxes as findings, padded by 10 % so hair and chin are covered.
pub fn face_findings(faces: &[Rect]) -> Vec<Finding> {
    faces
        .iter()
        .map(|r| {
            let r = r.normalized();
            Finding { category: Category::Face, rects: vec![r.inset(-r.width() * 0.1, -r.height() * 0.1)] }
        })
        .collect()
}

/// Final redaction rectangles: findings scaled from pixels to points
/// (`÷ scale`), clipped to `canvas`, with near-duplicates and rects inside
/// another one dropped.
pub fn redaction_rects(findings: &[Finding], scale: f64, canvas: Rect) -> Vec<Rect> {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let mut out: Vec<Rect> = Vec::new();
    let mut all: Vec<Rect> = findings
        .iter()
        .flat_map(|f| f.rects.iter())
        .filter_map(|r| {
            let r = r.normalized();
            Rect::new(r.min_x() / s, r.min_y() / s, r.width() / s, r.height() / s).intersection(&canvas)
        })
        .filter(|r| r.width() >= 1.0 && r.height() >= 1.0)
        .collect();
    // Biggest first so contained ones are dropped.
    all.sort_by(|a, b| b.area().total_cmp(&a.area()));
    for r in all {
        if !out.iter().any(|o| o.contains_rect(&r)) {
            out.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tb(t: &str, x: f64, y: f64, w: f64) -> TextBox {
        TextBox::new(t, Rect::new(x, y, w, 20.0))
    }

    #[test]
    fn settings_default_and_old_files_load() {
        let s: RedactSettings = toml::from_str("").unwrap();
        assert_eq!(s, RedactSettings::default());
        assert!(s.email && s.faces && !s.ai_check && !s.auto_redact && s.mode == RedactMode::Pixelate);
        let s: RedactSettings = toml::from_str("phone = false\nmode = \"blur\"").unwrap();
        assert!(!s.phone && s.email && s.mode == RedactMode::Blur);
        let mut s = RedactSettings::default();
        for c in Category::TEXT {
            s.set(c, false);
            assert!(!s.enabled(c));
        }
        assert!(s.enabled(Category::Other) && s.enabled(Category::Face));
    }

    #[test]
    fn finds_and_maps_text() {
        let boxes =
            [tb("Email: jane@example.com", 0.0, 0.0, 230.0), tb("Version 2.4.1 · 2024-05-01", 0.0, 40.0, 260.0)];
        let f = find_in_text(&boxes, &RedactSettings::default());
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].category, Category::Email);
        let r = f[0].rects[0];
        assert!(r.min_x() > 50.0 && r.min_x() < 80.0 && r.max_x() > 225.0);
        let off = RedactSettings { email: false, ..Default::default() };
        assert!(find_in_text(&boxes, &off).is_empty());
    }

    #[test]
    fn ai_lines_and_faces_become_findings() {
        let lines = ocr_lines(&[tb("John Smith", 0.0, 0.0, 100.0), tb("12 Baker Street", 0.0, 40.0, 150.0)]);
        let f = findings_for_lines(&lines, &[(1, Category::Other), (7, Category::Other)], &[]);
        assert_eq!(f.len(), 1);
        assert!(f[0].rects[0].contains_rect(&lines[1].rect));
        let faces = face_findings(&[Rect::new(10.0, 10.0, 100.0, 100.0)]);
        assert_eq!(faces[0].rects[0], Rect::new(0.0, 0.0, 120.0, 120.0));
    }

    #[test]
    fn rects_scale_clip_and_dedupe() {
        let findings = vec![
            Finding { category: Category::Face, rects: vec![Rect::new(-20.0, 0.0, 200.0, 100.0)] },
            Finding { category: Category::Email, rects: vec![Rect::new(10.0, 10.0, 20.0, 20.0)] },
            Finding { category: Category::Phone, rects: vec![Rect::new(300.0, 300.0, 40.0, 20.0)] },
        ];
        let rects = redaction_rects(&findings, 2.0, Rect::new(0.0, 0.0, 500.0, 500.0));
        assert_eq!(rects, vec![Rect::new(0.0, 0.0, 90.0, 50.0), Rect::new(150.0, 150.0, 20.0, 10.0)]);
    }
}
