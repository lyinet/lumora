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

/// Lowest opacity the style bar offers, in percent (fully invisible ink
/// would be a trap: you couldn't see what to select).
pub const OPACITY_MIN: u8 = 10;
/// Fully opaque, in percent.
pub const OPACITY_MAX: u8 = 100;

/// Clamp an opacity percentage into [`OPACITY_MIN`]…[`OPACITY_MAX`].
pub fn clamp_opacity(percent: u8) -> u8 {
    percent.clamp(OPACITY_MIN, OPACITY_MAX)
}

/// The inside of a rectangle or ellipse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Fill {
    /// Outline only (the classic look).
    #[default]
    None,
    /// Filled with the ink colour.
    Solid,
    /// Filled with the ink colour at [`FILL_TRANSLUCENT_ALPHA`].
    Translucent,
}

/// Alpha of a translucent fill (before the annotation's opacity).
pub const FILL_TRANSLUCENT_ALPHA: f64 = 0.3;

impl Fill {
    pub const ALL: [Fill; 3] = [Fill::None, Fill::Solid, Fill::Translucent];

    /// Alpha of the fill paint, or `None` for no fill.
    pub fn alpha(self) -> Option<f64> {
        match self {
            Fill::None => None,
            Fill::Solid => Some(1.0),
            Fill::Translucent => Some(FILL_TRANSLUCENT_ALPHA),
        }
    }
}

/// Colour, width, opacity and the per-kind looks, shared by every
/// annotation kind. Fields a kind doesn't use are ignored by it (a pen
/// stroke has no fill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub color: Rgb8,
    pub width: StrokeWidth,
    /// Opacity of the whole annotation in percent, 10–100. Redactions
    /// ignore it (a see-through redaction would leak what it hides).
    pub opacity: u8,
    /// Rectangles and ellipses only.
    pub fill: Fill,
}

impl Default for Style {
    fn default() -> Self {
        Style::new(PALETTE[0], StrokeWidth::Medium)
    }
}

impl Style {
    /// A style with `color` and `width` and every other field at its
    /// default (opaque, unfilled …).
    pub const fn new(color: Rgb8, width: StrokeWidth) -> Style {
        Style { color, width, opacity: OPACITY_MAX, fill: Fill::None }
    }

    /// Opacity as a 0.1–1.0 multiplier (clamped, so a bad value from a
    /// file can't make ink vanish).
    pub fn alpha(&self) -> f64 {
        f64::from(clamp_opacity(self.opacity)) / 100.0
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

    #[test]
    fn opacity_is_clamped_into_its_range() {
        assert_eq!(Style::default().alpha(), 1.0);
        assert_eq!(Style::new(PALETTE[1], StrokeWidth::Thin).opacity, 100);
        let mut s = Style { opacity: 0, ..Style::default() };
        assert_eq!(s.alpha(), 0.1);
        s.opacity = 255;
        assert_eq!(s.alpha(), 1.0);
        s.opacity = 55;
        assert!((s.alpha() - 0.55).abs() < 1e-12);
        assert_eq!(clamp_opacity(5), OPACITY_MIN);
        assert_eq!(clamp_opacity(70), 70);
    }

    #[test]
    fn fills() {
        assert_eq!(Style::default().fill, Fill::None);
        assert_eq!(Fill::None.alpha(), None);
        assert_eq!(Fill::Solid.alpha(), Some(1.0));
        assert!(Fill::Translucent.alpha().unwrap() < 0.5);
    }
}
