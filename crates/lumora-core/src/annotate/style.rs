//! Annotation styling: the curated colour palette and the three stroke
//! widths, plus the per-tool sizes derived from them.

use crate::color::Rgb8;

/// The six curated swatches, in toolbar order. Chosen to stay legible on
/// both light and dark screenshots; white and near-black cover the cases a
/// saturated colour can't.
pub const PALETTE: [Rgb8; 6] = [
    Rgb8::new(0xFF, 0x3B, 0x30), // red
    Rgb8::new(0xFF, 0xCC, 0x00), // yellow
    Rgb8::new(0x34, 0xC7, 0x59), // green
    Rgb8::new(0x0A, 0x84, 0xFF), // blue
    Rgb8::new(0x1C, 0x1C, 0x1E), // ink
    Rgb8::new(0xFF, 0xFF, 0xFF), // white
];

/// Thin / medium / thick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StrokeWidth {
    Thin,
    #[default]
    Medium,
    Thick,
}

impl StrokeWidth {
    pub const ALL: [StrokeWidth; 3] = [StrokeWidth::Thin, StrokeWidth::Medium, StrokeWidth::Thick];

    /// Outline width in points for arrows, lines, shapes and the pen.
    pub fn points(self) -> f64 {
        match self {
            StrokeWidth::Thin => 2.0,
            StrokeWidth::Medium => 4.0,
            StrokeWidth::Thick => 8.0,
        }
    }

    /// The highlighter is a broad marker.
    pub fn highlighter_points(self) -> f64 {
        match self {
            StrokeWidth::Thin => 12.0,
            StrokeWidth::Medium => 20.0,
            StrokeWidth::Thick => 32.0,
        }
    }

    /// Text size in points.
    pub fn font_size(self) -> f64 {
        match self {
            StrokeWidth::Thin => 15.0,
            StrokeWidth::Medium => 22.0,
            StrokeWidth::Thick => 34.0,
        }
    }

    /// Diameter of a numbered step badge.
    pub fn badge_diameter(self) -> f64 {
        match self {
            StrokeWidth::Thin => 24.0,
            StrokeWidth::Medium => 32.0,
            StrokeWidth::Thick => 44.0,
        }
    }

    /// Index 0–2 (toolbar segment, keyboard 1/2/3).
    pub fn index(self) -> usize {
        StrokeWidth::ALL.iter().position(|w| *w == self).unwrap_or(1)
    }
}

/// Colour + width, shared by every annotation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub color: Rgb8,
    pub width: StrokeWidth,
}

impl Default for Style {
    fn default() -> Self {
        Style { color: PALETTE[0], width: StrokeWidth::Medium }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_strictly_increasing() {
        for pair in StrokeWidth::ALL.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(a.points() < b.points());
            assert!(a.highlighter_points() < b.highlighter_points());
            assert!(a.font_size() < b.font_size());
            assert!(a.badge_diameter() < b.badge_diameter());
        }
        assert_eq!(StrokeWidth::Thick.index(), 2);
    }

    #[test]
    fn palette_is_distinct_and_spans_light_and_dark() {
        for (i, a) in PALETTE.iter().enumerate() {
            assert!(PALETTE.iter().skip(i + 1).all(|b| b != a));
        }
        assert!(PALETTE.iter().any(|c| c.prefers_dark_text()));
        assert!(PALETTE.iter().any(|c| !c.prefers_dark_text()));
        assert_eq!(Style::default().color, PALETTE[0]);
    }
}
