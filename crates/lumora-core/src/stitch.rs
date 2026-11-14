//! Planning a capture that spans several displays.
//!
//! Each display was frozen separately, possibly at different backing
//! scales. A selection crossing display edges is rendered into one bitmap
//! at the highest scale involved; every display contributes the part of the
//! selection it covers. Areas no display covers (the gaps of an L-shaped
//! arrangement) stay transparent.

use crate::geometry::Rect;

/// One frozen display: its global frame (points) and pixels per point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    pub frame: Rect,
    pub scale: f64,
}

/// What to copy from one source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    /// Index into the sources slice.
    pub source: usize,
    /// Region to crop, in global points.
    pub area: Rect,
    /// Where it lands in the output, in output pixels (y down).
    pub dest: Rect,
}

/// The output bitmap and its pieces.
#[derive(Debug, Clone, PartialEq)]
pub struct StitchPlan {
    /// The selection, aligned outward to whole output pixels (global points).
    pub area: Rect,
    pub scale: f64,
    pub width: u32,
    pub height: u32,
    pub pieces: Vec<Piece>,
}

impl StitchPlan {
    /// True when one display covers the whole selection (a plain crop is
    /// enough).
    pub fn is_single(&self) -> bool {
        self.pieces.len() == 1 && self.pieces[0].area == self.area
    }
}

/// Plan the composite for `area` (global points). `None` if no display
/// intersects it.
pub fn plan(area: Rect, sources: &[Source]) -> Option<StitchPlan> {
    let area = area.normalized();
    let hits: Vec<(usize, Rect)> =
        sources.iter().enumerate().filter_map(|(i, s)| area.intersection(&s.frame).map(|r| (i, r))).collect();
    if hits.is_empty() {
        return None;
    }
    let scale = hits.iter().map(|(i, _)| sources[*i].scale).fold(1.0, f64::max);
    let aligned = area.integral(scale);
    let (width, height) = aligned.pixel_size(scale);
    let pieces = hits
        .into_iter()
        .filter_map(|(i, _)| {
            let part = aligned.intersection(&sources[i].frame)?;
            let local = part.relative_to(&aligned);
            let dest =
                Rect::new(local.min_x() * scale, local.min_y() * scale, local.width() * scale, local.height() * scale)
                    .rounded(1.0);
            Some(Piece { source: i, area: part, dest })
        })
        .collect();
    Some(StitchPlan { area: aligned, scale, width, height, pieces })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn displays() -> Vec<Source> {
        vec![
            Source { frame: Rect::new(0.0, 0.0, 1000.0, 800.0), scale: 2.0 },
            Source { frame: Rect::new(1000.0, 0.0, 1920.0, 1080.0), scale: 1.0 },
        ]
    }

    #[test]
    fn inside_one_display_is_a_single_piece() {
        let p = plan(Rect::new(100.0, 100.0, 200.0, 100.0), &displays()).unwrap();
        assert!(p.is_single());
        assert_eq!((p.width, p.height, p.scale), (400, 200, 2.0));
        assert_eq!(p.pieces[0].dest, Rect::new(0.0, 0.0, 400.0, 200.0));
    }

    #[test]
    fn spanning_selection_uses_the_highest_scale_and_abuts() {
        let p = plan(Rect::new(900.0, 100.0, 200.0, 100.0), &displays()).unwrap();
        assert!(!p.is_single());
        assert_eq!((p.width, p.height, p.scale), (400, 200, 2.0));
        assert_eq!(p.pieces.len(), 2);
        let (a, b) = (p.pieces[0], p.pieces[1]);
        assert_eq!(a.area, Rect::new(900.0, 100.0, 100.0, 100.0));
        assert_eq!(b.area, Rect::new(1000.0, 100.0, 100.0, 100.0));
        assert_eq!(a.dest, Rect::new(0.0, 0.0, 200.0, 200.0));
        assert_eq!(b.dest, Rect::new(200.0, 0.0, 200.0, 200.0));
    }

    #[test]
    fn uncovered_regions_are_left_out() {
        // Extends below the shorter left display.
        let p = plan(Rect::new(900.0, 700.0, 200.0, 200.0), &displays()).unwrap();
        assert_eq!(p.pieces[0].area, Rect::new(900.0, 700.0, 100.0, 100.0));
        assert_eq!(p.pieces[1].area, Rect::new(1000.0, 700.0, 100.0, 200.0));
        let covered: f64 = p.pieces.iter().map(|x| x.dest.area()).sum();
        assert!(covered < f64::from(p.width * p.height));
    }

    #[test]
    fn nothing_under_the_selection() {
        assert_eq!(plan(Rect::new(-500.0, -500.0, 10.0, 10.0), &displays()), None);
    }

    #[test]
    fn fractional_selections_align_outward() {
        let p = plan(Rect::new(10.3, 10.3, 5.1, 5.1), &displays()).unwrap();
        assert_eq!(p.area, Rect::new(10.0, 10.0, 5.5, 5.5));
        assert_eq!((p.width, p.height), (11, 11));
    }
}
