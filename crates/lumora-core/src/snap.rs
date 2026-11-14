//! Window hit-testing and magnetic edge snapping for the capture overlay.
//!
//! The capture layer fetches window frames from ScreenCaptureKit (already in
//! global top-left space, frontmost first) and hands them to [`WindowList`].
//! Hovering then highlights the topmost window under the cursor, and dragging
//! a selection "sticks" to nearby window and display edges.

use crate::geometry::{Point, Rect};

/// A capturable on-screen window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    /// `CGWindowID` / `SCWindow.windowID`.
    pub id: u32,
    /// Global top-left frame in points.
    pub frame: Rect,
    pub app_name: String,
    pub title: String,
    /// Window layer; 0 is the normal application layer.
    pub layer: i32,
}

/// Windows smaller than this (points, on either axis) are ignored — they are
/// usually invisible helper windows or status items.
pub const MIN_WINDOW_SIDE: f64 = 40.0;

/// Front-to-back list of pickable windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowList {
    windows: Vec<WindowInfo>,
}

impl WindowList {
    /// Build from windows ordered **frontmost first**. Non-normal layers and
    /// tiny windows are filtered out.
    pub fn new(windows: impl IntoIterator<Item = WindowInfo>) -> Self {
        WindowList {
            windows: windows
                .into_iter()
                .filter(|w| w.layer == 0 && w.frame.width() >= MIN_WINDOW_SIDE && w.frame.height() >= MIN_WINDOW_SIDE)
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&WindowInfo> {
        self.windows.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &WindowInfo> {
        self.windows.iter()
    }

    /// Index of the frontmost window containing `p`.
    pub fn index_at(&self, p: Point) -> Option<usize> {
        self.windows.iter().position(|w| w.frame.contains(p))
    }

    /// The frontmost window containing `p`.
    pub fn window_at(&self, p: Point) -> Option<&WindowInfo> {
        self.index_at(p).map(|i| &self.windows[i])
    }

    /// The visible (unoccluded-bounding-box) part of window `index` clipped to
    /// `bounds`, for drawing the highlight. Occlusion by windows in front is
    /// not subtracted — the highlight shows the whole window like macOS does.
    pub fn highlight_rect(&self, index: usize, bounds: &Rect) -> Option<Rect> {
        self.windows.get(index)?.frame.intersection(bounds)
    }

    /// Candidate snap lines: every window edge plus the edges of `extra`
    /// rectangles (typically the display bounds).
    pub fn edges(&self, extra: &[Rect]) -> SnapEdges {
        let mut e = SnapEdges::default();
        for r in self.windows.iter().map(|w| &w.frame).chain(extra.iter()) {
            e.xs.extend([r.min_x(), r.max_x()]);
            e.ys.extend([r.min_y(), r.max_y()]);
        }
        e.xs.sort_by(f64::total_cmp);
        e.xs.dedup();
        e.ys.sort_by(f64::total_cmp);
        e.ys.dedup();
        e
    }
}

/// Sorted vertical (`xs`) and horizontal (`ys`) snap lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapEdges {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
}

impl SnapEdges {
    /// Snap `p` independently on each axis to the nearest line within
    /// `threshold`.
    pub fn snap(&self, p: Point, threshold: f64) -> Point {
        Point::new(snap_value(p.x, &self.xs, threshold), snap_value(p.y, &self.ys, threshold))
    }
}

/// Return the candidate nearest to `v` if within `threshold`, else `v`.
/// `candidates` must be sorted ascending.
pub fn snap_value(v: f64, candidates: &[f64], threshold: f64) -> f64 {
    let i = candidates.partition_point(|&c| c < v);
    let mut best = v;
    let mut best_d = threshold;
    for c in candidates[i.saturating_sub(1)..(i + 1).min(candidates.len())].iter() {
        let d = (c - v).abs();
        if d <= best_d {
            best = *c;
            best_d = d;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u32, r: Rect) -> WindowInfo {
        WindowInfo { id, frame: r, app_name: format!("App{id}"), title: String::new(), layer: 0 }
    }

    fn sample() -> WindowList {
        WindowList::new([
            win(1, Rect::new(100.0, 100.0, 200.0, 200.0)), // front
            win(2, Rect::new(0.0, 0.0, 400.0, 400.0)),     // back
            win(3, Rect::new(500.0, 0.0, 10.0, 10.0)),     // tiny, filtered
            WindowInfo { layer: 25, ..win(4, Rect::new(0.0, 0.0, 1000.0, 30.0)) }, // menu bar layer
        ])
    }

    #[test]
    fn filters_and_orders() {
        let l = sample();
        assert_eq!(l.len(), 2);
        assert_eq!(l.window_at(Point::new(150.0, 150.0)).map(|w| w.id), Some(1));
        assert_eq!(l.window_at(Point::new(50.0, 50.0)).map(|w| w.id), Some(2));
        assert_eq!(l.window_at(Point::new(505.0, 5.0)), None);
        assert_eq!(l.window_at(Point::new(900.0, 900.0)), None);
    }

    #[test]
    fn highlight_is_clipped() {
        let l = sample();
        let screen = Rect::new(0.0, 0.0, 250.0, 250.0);
        assert_eq!(l.highlight_rect(0, &screen), Some(Rect::new(100.0, 100.0, 150.0, 150.0)));
        assert_eq!(l.highlight_rect(9, &screen), None);
    }

    #[test]
    fn snapping() {
        let l = sample();
        let e = l.edges(&[Rect::new(0.0, 0.0, 1440.0, 900.0)]);
        assert_eq!(e.xs, vec![0.0, 100.0, 300.0, 400.0, 1440.0]);
        assert_eq!(e.snap(Point::new(104.0, 397.0), 6.0), Point::new(100.0, 400.0));
        assert_eq!(e.snap(Point::new(110.0, 200.0), 6.0), Point::new(110.0, 200.0));
        assert_eq!(snap_value(5.0, &[], 10.0), 5.0);
        assert_eq!(snap_value(50.0, &[0.0, 52.0, 53.0], 10.0), 52.0);
    }
}
