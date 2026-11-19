//! Annotation geometry: what each kind stores, its bounds, hit-testing,
//! translation and handle-driven resizing.

use crate::geometry::{Handle, Point, Rect, Size};
use crate::selection::constrain_aspect;

use super::path::{
    arrow_centerline, arrow_outline, distance_to_polyline, point_in_polygon, quad_point, Path, CURVE_BULGE,
};
use super::style::{Fill, Style};

/// How a redaction hides what's under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RedactMode {
    #[default]
    Pixelate,
    Blur,
}

/// The geometry of one annotation, in document points (y down, origin at
/// the top-left of the uncropped image).
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// `control` makes it a quadratic curve.
    Arrow {
        start: Point,
        end: Point,
        control: Option<Point>,
    },
    Line {
        start: Point,
        end: Point,
    },
    Rect {
        rect: Rect,
        rounded: bool,
    },
    Ellipse {
        rect: Rect,
    },
    /// Raw pointer samples; smoothed when rendered.
    Pen {
        points: Vec<Point>,
    },
    Highlighter {
        points: Vec<Point>,
    },
    /// `size` is the measured text size (set by the UI, which owns the
    /// fonts); zero means "estimate".
    Text {
        origin: Point,
        text: String,
        size: Size,
        background: bool,
    },
    /// A numbered badge. The number is derived from creation order by the
    /// document, so deleting step 2 renumbers 3 → 2.
    Step {
        center: Point,
    },
    Redact {
        rect: Rect,
        mode: RedactMode,
    },
    /// Dim everything except this region.
    Spotlight {
        rect: Rect,
        ellipse: bool,
    },
}

/// Something the user can drag to reshape an annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditHandle {
    Start,
    End,
    /// The curve control point of a curved arrow.
    Control,
    Box(Handle),
}

/// Padding around text drawn on a background pill, as fractions of the
/// font size (horizontal, vertical).
pub const TEXT_PILL_PAD: (f64, f64) = (0.4, 0.2);

/// Rough text metrics for when the UI hasn't measured yet: CJK glyphs are
/// about 1 em wide, Latin about 0.58 em; lines are 1.25 em.
pub fn estimate_text_size(text: &str, font_size: f64) -> Size {
    let lines: Vec<&str> = if text.is_empty() { vec![""] } else { text.split('\n').collect() };
    let width = lines
        .iter()
        .map(|l| l.chars().map(|c| if (c as u32) >= 0x2E80 { 1.0 } else { 0.58 }).sum::<f64>())
        .fold(0.0, f64::max)
        * font_size;
    Size::new(width.max(font_size * 0.5), lines.len() as f64 * font_size * 1.25)
}

impl Shape {
    /// Whether this is a two-point shape (endpoint handles).
    pub fn is_linear(&self) -> bool {
        matches!(self, Shape::Arrow { .. } | Shape::Line { .. })
    }

    /// The text box of a text annotation (without the pill padding).
    pub fn text_rect(&self, style: &Style) -> Option<Rect> {
        match self {
            Shape::Text { origin, text, size, .. } => {
                let s = if size.width > 0.0 && size.height > 0.0 {
                    *size
                } else {
                    estimate_text_size(text, style.width.font_size())
                };
                Some(Rect { origin: *origin, size: s })
            }
            _ => None,
        }
    }

    /// Geometric bounds (what selection chrome is drawn around), without
    /// stroke width.
    pub fn bounds(&self, style: &Style) -> Rect {
        match self {
            Shape::Arrow { start, end, control } => {
                let pts = arrow_centerline(*start, *end, *control);
                bounds_of(&pts)
            }
            Shape::Line { start, end } => Rect::from_corners(*start, *end),
            Shape::Rect { rect, .. } | Shape::Ellipse { rect } | Shape::Redact { rect, .. } => rect.normalized(),
            Shape::Spotlight { rect, .. } => rect.normalized(),
            Shape::Pen { points } | Shape::Highlighter { points } => bounds_of(points),
            Shape::Text { background, .. } => {
                let r = self.text_rect(style).unwrap_or_default();
                if *background {
                    let f = style.width.font_size();
                    r.inset(-f * TEXT_PILL_PAD.0, -f * TEXT_PILL_PAD.1)
                } else {
                    r
                }
            }
            Shape::Step { center } => {
                let r = style.width.badge_diameter() / 2.0;
                Rect::new(center.x - r, center.y - r, r * 2.0, r * 2.0)
            }
        }
    }

    /// Effective stroke width for hit-testing (0 for filled kinds).
    fn stroke(&self, style: &Style) -> f64 {
        match self {
            Shape::Highlighter { .. } => style.width.highlighter_points(),
            Shape::Arrow { .. }
            | Shape::Line { .. }
            | Shape::Rect { .. }
            | Shape::Ellipse { .. }
            | Shape::Pen { .. } => style.width.points(),
            _ => 0.0,
        }
    }

    /// Whether `p` touches the annotation. Outlined shapes are hit on their
    /// stroke only (so you can draw inside a box); filled kinds anywhere
    /// inside. `slop` widens the target (a few screen points / zoom).
    pub fn hit(&self, p: Point, style: &Style, slop: f64) -> bool {
        let reach = self.stroke(style) / 2.0 + slop;
        match self {
            Shape::Arrow { start, end, control } => {
                let line = arrow_centerline(*start, *end, *control);
                if distance_to_polyline(p, &line) <= reach {
                    return true;
                }
                let outline = arrow_outline(*start, *end, *control, style.width.points());
                outline.flatten(1).first().is_some_and(|poly| point_in_polygon(p, poly))
            }
            Shape::Line { start, end } => distance_to_polyline(p, &[*start, *end]) <= reach,
            Shape::Rect { rect, rounded } => {
                let path = if *rounded { Path::rounded_rect(*rect, rect_radius(style)) } else { Path::rect(*rect) };
                outline_hit(&path, 4, p, reach, style.fill != Fill::None)
            }
            Shape::Ellipse { rect } => outline_hit(&Path::ellipse(*rect), 16, p, reach, style.fill != Fill::None),
            Shape::Pen { points } | Shape::Highlighter { points } => distance_to_polyline(p, points) <= reach,
            Shape::Step { center } => p.distance(*center) <= style.width.badge_diameter() / 2.0 + slop,
            Shape::Text { .. } | Shape::Redact { .. } | Shape::Spotlight { .. } => {
                self.bounds(style).inset(-slop, -slop).contains(p)
            }
        }
    }

    /// Move by `(dx, dy)`.
    pub fn translated(&self, dx: f64, dy: f64) -> Shape {
        let m = |p: Point| Point::new(p.x + dx, p.y + dy);
        match self {
            Shape::Arrow { start, end, control } => {
                Shape::Arrow { start: m(*start), end: m(*end), control: control.map(m) }
            }
            Shape::Line { start, end } => Shape::Line { start: m(*start), end: m(*end) },
            Shape::Rect { rect, rounded } => Shape::Rect { rect: rect.offset(dx, dy), rounded: *rounded },
            Shape::Ellipse { rect } => Shape::Ellipse { rect: rect.offset(dx, dy) },
            Shape::Pen { points } => Shape::Pen { points: points.iter().copied().map(m).collect() },
            Shape::Highlighter { points } => Shape::Highlighter { points: points.iter().copied().map(m).collect() },
            Shape::Text { origin, text, size, background } => {
                Shape::Text { origin: m(*origin), text: text.clone(), size: *size, background: *background }
            }
            Shape::Step { center } => Shape::Step { center: m(*center) },
            Shape::Redact { rect, mode } => Shape::Redact { rect: rect.offset(dx, dy), mode: *mode },
            Shape::Spotlight { rect, ellipse } => Shape::Spotlight { rect: rect.offset(dx, dy), ellipse: *ellipse },
        }
    }

    /// The box this shape is resized through, for box-handled kinds.
    fn box_rect(&self, style: &Style) -> Option<Rect> {
        match self {
            Shape::Rect { .. }
            | Shape::Ellipse { .. }
            | Shape::Redact { .. }
            | Shape::Spotlight { .. }
            | Shape::Pen { .. }
            | Shape::Highlighter { .. } => Some(self.bounds(style)),
            _ => None,
        }
    }

    /// The handles shown when selected, with their positions.
    pub fn handles(&self, style: &Style) -> Vec<(EditHandle, Point)> {
        match self {
            Shape::Arrow { start, end, control } => {
                let mut v = vec![(EditHandle::Start, *start), (EditHandle::End, *end)];
                if let Some(c) = control {
                    // The handle sits on the curve (t = ½), not at the
                    // control point, which can be far off the stroke.
                    v.push((EditHandle::Control, quad_point(*start, *c, *end, 0.5)));
                }
                v
            }
            Shape::Line { start, end } => vec![(EditHandle::Start, *start), (EditHandle::End, *end)],
            Shape::Text { .. } | Shape::Step { .. } => Vec::new(),
            _ => self
                .box_rect(style)
                .map(|r| Handle::ALL.iter().map(|h| (EditHandle::Box(*h), r.handle_point(*h))).collect())
                .unwrap_or_default(),
        }
    }

    /// The handle under `p` (within `radius`), if any.
    pub fn handle_at(&self, p: Point, style: &Style, radius: f64) -> Option<EditHandle> {
        self.handles(style)
            .into_iter()
            .filter(|(_, at)| (p.x - at.x).abs() <= radius && (p.y - at.y).abs() <= radius)
            .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
            .map(|(h, _)| h)
    }

    /// Reshape by dragging `handle` to `p`. `original` is the shape when the
    /// drag began (resizing is always relative to it, so crossing over
    /// flips cleanly). `constrain` (Shift) snaps lines to 45° and keeps box
    /// aspect ratios.
    pub fn with_handle(original: &Shape, handle: EditHandle, p: Point, style: &Style, constrain: bool) -> Shape {
        use super::path::snap_angle;
        match (original, handle) {
            (Shape::Arrow { start, end, control }, EditHandle::Start | EditHandle::End) => {
                let moving_start = handle == EditHandle::Start;
                let anchor = if moving_start { *end } else { *start };
                let q = if constrain { snap_angle(anchor, p) } else { p };
                let old = if moving_start { *start } else { *end };
                // Keep the bend: the control point follows half the move.
                let control = control.map(|c| Point::new(c.x + (q.x - old.x) / 2.0, c.y + (q.y - old.y) / 2.0));
                if moving_start {
                    Shape::Arrow { start: q, end: *end, control }
                } else {
                    Shape::Arrow { start: *start, end: q, control }
                }
            }
            (Shape::Arrow { start, end, .. }, EditHandle::Control) => {
                // The handle is the curve's midpoint M = (S + 2C + E) / 4,
                // so C = 2M - (S + E) / 2.
                let c = Point::new(2.0 * p.x - (start.x + end.x) / 2.0, 2.0 * p.y - (start.y + end.y) / 2.0);
                Shape::Arrow { start: *start, end: *end, control: Some(c) }
            }
            (Shape::Line { end, .. }, EditHandle::Start) => {
                Shape::Line { start: if constrain { snap_angle(*end, p) } else { p }, end: *end }
            }
            (Shape::Line { start, .. }, EditHandle::End) => {
                Shape::Line { start: *start, end: if constrain { snap_angle(*start, p) } else { p } }
            }
            (_, EditHandle::Box(h)) => match original.box_rect(style) {
                Some(r) => {
                    let new = resize_rect(r, h, p, constrain);
                    original.fitted_to(r, new)
                }
                None => original.clone(),
            },
            _ => original.clone(),
        }
    }

    /// Map the shape from box `from` onto box `to`.
    fn fitted_to(&self, from: Rect, to: Rect) -> Shape {
        match self {
            Shape::Rect { rounded, .. } => Shape::Rect { rect: to, rounded: *rounded },
            Shape::Ellipse { .. } => Shape::Ellipse { rect: to },
            Shape::Redact { mode, .. } => Shape::Redact { rect: to, mode: *mode },
            Shape::Spotlight { ellipse, .. } => Shape::Spotlight { rect: to, ellipse: *ellipse },
            Shape::Pen { points } => Shape::Pen { points: map_points(points, from, to) },
            Shape::Highlighter { points } => Shape::Highlighter { points: map_points(points, from, to) },
            other => other.clone(),
        }
    }

    /// Big enough to keep after a creation drag?
    pub fn is_meaningful(&self, style: &Style) -> bool {
        const MIN: f64 = 3.0;
        match self {
            Shape::Arrow { start, end, .. } | Shape::Line { start, end } => start.distance(*end) >= MIN,
            Shape::Pen { points } | Shape::Highlighter { points } => !points.is_empty(),
            Shape::Text { text, .. } => !text.trim().is_empty(),
            Shape::Step { .. } => true,
            _ => {
                let b = self.bounds(style);
                b.width() >= MIN && b.height() >= MIN
            }
        }
    }

    /// Give a straight arrow a gentle default bend, or straighten a curved
    /// one (the toolbar's "curved" toggle).
    pub fn with_curve(&self, curved: bool) -> Shape {
        match self {
            Shape::Arrow { start, end, control } => {
                let control = match (curved, control) {
                    (true, None) => Some(default_control(*start, *end)),
                    (true, Some(c)) => Some(*c),
                    (false, _) => None,
                };
                Shape::Arrow { start: *start, end: *end, control }
            }
            other => other.clone(),
        }
    }
}

/// Hit on a closed outline's stroke, or anywhere inside when it is filled.
fn outline_hit(path: &Path, steps: usize, p: Point, reach: f64, filled: bool) -> bool {
    let polys = path.flatten(steps);
    polys.iter().any(|poly| distance_to_polyline(p, poly) <= reach)
        || (filled && polys.iter().any(|poly| point_in_polygon(p, poly)))
}

/// Corner radius used for rounded rectangles.
pub fn rect_radius(style: &Style) -> f64 {
    (style.width.points() * 2.0).max(8.0)
}

/// Default control point of a curved arrow: a bulge to the left of the
/// direction of travel, proportional to its length.
pub fn default_control(start: Point, end: Point) -> Point {
    let mid = start.lerp(end, 0.5);
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    Point::new(mid.x + dy * CURVE_BULGE, mid.y - dx * CURVE_BULGE)
}

/// Resize `r` by dragging handle `h` to `p`; the opposite handle is fixed.
pub fn resize_rect(r: Rect, h: Handle, p: Point, constrain: bool) -> Rect {
    let fixed = r.handle_point(h.opposite());
    match h.axes() {
        (false, _) => Rect::from_corners(Point::new(r.min_x(), fixed.y), Point::new(r.max_x(), p.y)),
        (_, false) => Rect::from_corners(Point::new(fixed.x, r.min_y()), Point::new(p.x, r.max_y())),
        _ => {
            let moving = match (constrain, r.size.aspect_ratio()) {
                (true, Some(ratio)) if ratio > 0.0 => constrain_aspect(fixed, p, ratio),
                _ => p,
            };
            Rect::from_corners(fixed, moving)
        }
    }
}

fn map_points(points: &[Point], from: Rect, to: Rect) -> Vec<Point> {
    let sx = if from.width() > 1e-9 { to.width() / from.width() } else { 1.0 };
    let sy = if from.height() > 1e-9 { to.height() / from.height() } else { 1.0 };
    points
        .iter()
        .map(|p| Point::new(to.min_x() + (p.x - from.min_x()) * sx, to.min_y() + (p.y - from.min_y()) * sy))
        .collect()
}

/// Bounding box of a point set (zero rect for none).
pub fn bounds_of(points: &[Point]) -> Rect {
    let Some(first) = points.first() else { return Rect::ZERO };
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
    for p in points {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::style::StrokeWidth;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn style() -> Style {
        Style { width: StrokeWidth::Medium, ..Style::default() }
    }

    #[test]
    fn line_hits_on_stroke_with_slop() {
        let s = Shape::Line { start: p(0.0, 0.0), end: p(100.0, 0.0) };
        assert!(s.hit(p(50.0, 2.0), &style(), 0.0));
        assert!(!s.hit(p(50.0, 5.0), &style(), 0.0));
        assert!(s.hit(p(50.0, 5.0), &style(), 4.0));
    }

    #[test]
    fn outlined_rect_is_hit_on_the_border_only() {
        let s = Shape::Rect { rect: Rect::new(0.0, 0.0, 100.0, 50.0), rounded: false };
        assert!(s.hit(p(0.0, 25.0), &style(), 1.0));
        assert!(s.hit(p(100.0, 49.0), &style(), 1.0));
        assert!(!s.hit(p(50.0, 25.0), &style(), 1.0));
        let r = Shape::Rect { rect: Rect::new(0.0, 0.0, 100.0, 50.0), rounded: true };
        // The very corner is cut off by the rounding.
        assert!(!r.hit(p(0.0, 0.0), &style(), 0.5));
        assert!(r.hit(p(50.0, 0.0), &style(), 0.5));
    }

    #[test]
    fn filled_rects_and_ellipses_hit_inside() {
        let filled = Style { fill: Fill::Translucent, ..style() };
        let r = Shape::Rect { rect: Rect::new(0.0, 0.0, 100.0, 50.0), rounded: true };
        assert!(!r.hit(p(50.0, 25.0), &style(), 1.0));
        assert!(r.hit(p(50.0, 25.0), &filled, 1.0));
        assert!(!r.hit(p(150.0, 25.0), &filled, 1.0));
        let e = Shape::Ellipse { rect: Rect::new(0.0, 0.0, 100.0, 50.0) };
        assert!(e.hit(p(50.0, 25.0), &Style { fill: Fill::Solid, ..style() }, 1.0));
        assert!(!e.hit(p(2.0, 2.0), &Style { fill: Fill::Solid, ..style() }, 1.0), "outside the curve");
    }

    #[test]
    fn ellipse_hits_its_outline() {
        let s = Shape::Ellipse { rect: Rect::new(0.0, 0.0, 100.0, 50.0) };
        assert!(s.hit(p(50.0, 0.0), &style(), 1.0));
        assert!(s.hit(p(100.0, 25.0), &style(), 1.0));
        assert!(!s.hit(p(50.0, 25.0), &style(), 1.0));
        assert!(!s.hit(p(0.0, 0.0), &style(), 1.0));
    }

    #[test]
    fn filled_kinds_hit_inside() {
        let r = Shape::Redact { rect: Rect::new(10.0, 10.0, 20.0, 20.0), mode: RedactMode::Blur };
        assert!(r.hit(p(20.0, 20.0), &style(), 0.0));
        assert!(!r.hit(p(40.0, 20.0), &style(), 0.0));
        let st = Shape::Step { center: p(0.0, 0.0) };
        assert!(st.hit(p(15.0, 0.0), &style(), 0.0));
        assert!(!st.hit(p(17.0, 0.0), &style(), 0.0));
    }

    #[test]
    fn arrow_hits_shaft_and_head() {
        let a = Shape::Arrow { start: p(0.0, 0.0), end: p(100.0, 0.0), control: None };
        assert!(a.hit(p(50.0, 1.0), &style(), 0.0));
        // The head is wider than the shaft.
        assert!(a.hit(p(85.0, 6.0), &style(), 0.0));
        assert!(!a.hit(p(50.0, 8.0), &style(), 0.0));
    }

    #[test]
    fn freehand_bounds_and_hit() {
        let s = Shape::Pen { points: vec![p(0.0, 0.0), p(10.0, 10.0), p(20.0, 0.0)] };
        assert_eq!(s.bounds(&style()), Rect::new(0.0, 0.0, 20.0, 10.0));
        assert!(s.hit(p(5.0, 5.0), &style(), 0.0));
        assert!(!s.hit(p(10.0, 0.0), &style(), 0.0));
        let h = Shape::Highlighter { points: vec![p(0.0, 0.0), p(100.0, 0.0)] };
        assert!(h.hit(p(50.0, 9.0), &style(), 0.0), "highlighter is 20 pt wide");
    }

    #[test]
    fn translation_moves_every_point() {
        let a = Shape::Arrow { start: p(0.0, 0.0), end: p(10.0, 0.0), control: Some(p(5.0, 5.0)) };
        assert_eq!(
            a.translated(1.0, 2.0),
            Shape::Arrow { start: p(1.0, 2.0), end: p(11.0, 2.0), control: Some(p(6.0, 7.0)) }
        );
        let t = Shape::Text { origin: p(0.0, 0.0), text: "hi".into(), size: Size::new(10.0, 10.0), background: true };
        assert!(matches!(t.translated(3.0, 4.0), Shape::Text { origin, .. } if origin == p(3.0, 4.0)));
    }

    #[test]
    fn box_handles_resize_and_flip() {
        let s = Shape::Rect { rect: Rect::new(0.0, 0.0, 100.0, 50.0), rounded: false };
        let r = Shape::with_handle(&s, EditHandle::Box(Handle::BottomRight), p(120.0, 80.0), &style(), false);
        assert_eq!(r, Shape::Rect { rect: Rect::new(0.0, 0.0, 120.0, 80.0), rounded: false });
        // Dragging the right edge past the left edge flips.
        let f = Shape::with_handle(&s, EditHandle::Box(Handle::Right), p(-20.0, 999.0), &style(), false);
        assert_eq!(f, Shape::Rect { rect: Rect::new(-20.0, 0.0, 20.0, 50.0), rounded: false });
        // Shift keeps the 2:1 aspect.
        let c = Shape::with_handle(&s, EditHandle::Box(Handle::BottomRight), p(200.0, 60.0), &style(), true);
        assert_eq!(c.bounds(&style()), Rect::new(0.0, 0.0, 200.0, 100.0));
    }

    #[test]
    fn pen_scales_with_its_box() {
        let s = Shape::Pen { points: vec![p(0.0, 0.0), p(10.0, 10.0)] };
        let r = Shape::with_handle(&s, EditHandle::Box(Handle::BottomRight), p(20.0, 40.0), &style(), false);
        assert_eq!(r, Shape::Pen { points: vec![p(0.0, 0.0), p(20.0, 40.0)] });
    }

    #[test]
    fn endpoint_handles_and_angle_snap() {
        let l = Shape::Line { start: p(0.0, 0.0), end: p(10.0, 0.0) };
        assert_eq!(l.handle_at(p(9.0, 1.0), &style(), 3.0), Some(EditHandle::End));
        assert_eq!(l.handle_at(p(5.0, 0.0), &style(), 3.0), None);
        let m = Shape::with_handle(&l, EditHandle::End, p(10.0, 1.0), &style(), true);
        match m {
            Shape::Line { end, .. } => assert!(end.y.abs() < 1e-9),
            _ => unreachable!(),
        }
    }

    #[test]
    fn curve_handle_sits_on_the_curve_and_round_trips() {
        let a = Shape::Arrow { start: p(0.0, 0.0), end: p(100.0, 0.0), control: None }.with_curve(true);
        let handles = a.handles(&style());
        assert_eq!(handles.len(), 3);
        let (h, at) = handles[2];
        assert_eq!(h, EditHandle::Control);
        // Dragging the handle to where it already is changes nothing.
        let same = Shape::with_handle(&a, EditHandle::Control, at, &style(), false);
        match (&a, &same) {
            (Shape::Arrow { control: Some(c1), .. }, Shape::Arrow { control: Some(c2), .. }) => {
                assert!(c1.distance(*c2) < 1e-9)
            }
            _ => unreachable!(),
        }
        // Dragging it to y = 30 puts the curve's apex there.
        let bent = Shape::with_handle(&a, EditHandle::Control, p(50.0, 30.0), &style(), false);
        if let Shape::Arrow { start, end, control: Some(c) } = bent {
            assert!((quad_point(start, c, end, 0.5).y - 30.0).abs() < 1e-9);
        }
        assert!(matches!(a.with_curve(false), Shape::Arrow { control: None, .. }));
    }

    #[test]
    fn moving_an_endpoint_keeps_the_bend() {
        let a = Shape::Arrow { start: p(0.0, 0.0), end: p(100.0, 0.0), control: Some(p(50.0, 40.0)) };
        let m = Shape::with_handle(&a, EditHandle::End, p(120.0, 0.0), &style(), false);
        assert_eq!(m, Shape::Arrow { start: p(0.0, 0.0), end: p(120.0, 0.0), control: Some(p(60.0, 40.0)) });
    }

    #[test]
    fn text_bounds_use_measurement_or_estimate() {
        let measured =
            Shape::Text { origin: p(10.0, 10.0), text: "Hello".into(), size: Size::new(50.0, 20.0), background: false };
        assert_eq!(measured.bounds(&style()), Rect::new(10.0, 10.0, 50.0, 20.0));
        let pill =
            Shape::Text { origin: p(10.0, 10.0), text: "Hello".into(), size: Size::new(50.0, 20.0), background: true };
        assert!(pill.bounds(&style()).contains_rect(&measured.bounds(&style())));
        let est = estimate_text_size("你好", 20.0);
        assert_eq!(est, Size::new(40.0, 25.0));
        let two = estimate_text_size("ab\nabcd", 10.0);
        assert!((two.width - 23.2).abs() < 1e-9 && two.height == 25.0);
    }

    #[test]
    fn meaningful_sizes() {
        let st = style();
        assert!(!Shape::Line { start: p(0.0, 0.0), end: p(1.0, 1.0) }.is_meaningful(&st));
        assert!(Shape::Line { start: p(0.0, 0.0), end: p(10.0, 0.0) }.is_meaningful(&st));
        assert!(!Shape::Rect { rect: Rect::new(0.0, 0.0, 100.0, 1.0), rounded: false }.is_meaningful(&st));
        assert!(Shape::Pen { points: vec![p(0.0, 0.0)] }.is_meaningful(&st));
        assert!(!Shape::Text { origin: p(0.0, 0.0), text: "  ".into(), size: Size::ZERO, background: false }
            .is_meaningful(&st));
    }
}
