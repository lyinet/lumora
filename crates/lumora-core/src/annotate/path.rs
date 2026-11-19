//! Vector paths and the geometry the annotation tools are built from:
//! Bézier sampling, distance queries, freehand smoothing and the tapered
//! arrow outline.
//!
//! A [`Path`] is a plain list of [`PathEl`]s in document points. The UI turns
//! it into a `CGPath` element by element, so a path built here renders the
//! same in a live `CAShapeLayer` and in the flattened export.

use crate::geometry::{Point, Rect};

/// Cubic Bézier constant for approximating a quarter circle.
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// One path element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathEl {
    MoveTo(Point),
    LineTo(Point),
    /// Quadratic Bézier: control, end.
    QuadTo(Point, Point),
    /// Cubic Bézier: control 1, control 2, end.
    CubicTo(Point, Point, Point),
    Close,
}

/// A vector path (possibly several sub-paths).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub els: Vec<PathEl>,
}

impl Path {
    pub fn new() -> Self {
        Path::default()
    }

    pub fn is_empty(&self) -> bool {
        self.els.is_empty()
    }

    pub fn move_to(&mut self, p: Point) -> &mut Self {
        self.els.push(PathEl::MoveTo(p));
        self
    }

    pub fn line_to(&mut self, p: Point) -> &mut Self {
        self.els.push(PathEl::LineTo(p));
        self
    }

    pub fn quad_to(&mut self, c: Point, p: Point) -> &mut Self {
        self.els.push(PathEl::QuadTo(c, p));
        self
    }

    pub fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) -> &mut Self {
        self.els.push(PathEl::CubicTo(c1, c2, p));
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.els.push(PathEl::Close);
        self
    }

    /// Append every element of `other` (as separate sub-paths).
    pub fn extend(&mut self, other: &Path) -> &mut Self {
        self.els.extend_from_slice(&other.els);
        self
    }

    /// A closed polygon through `points`.
    pub fn polygon(points: &[Point]) -> Path {
        let mut p = Path::new();
        if let Some((first, rest)) = points.split_first() {
            p.move_to(*first);
            for q in rest {
                p.line_to(*q);
            }
            p.close();
        }
        p
    }

    /// An open polyline through `points`.
    pub fn polyline(points: &[Point]) -> Path {
        let mut p = Path::new();
        if let Some((first, rest)) = points.split_first() {
            p.move_to(*first);
            for q in rest {
                p.line_to(*q);
            }
        }
        p
    }

    pub fn rect(r: Rect) -> Path {
        Path::polygon(&[
            Point::new(r.min_x(), r.min_y()),
            Point::new(r.max_x(), r.min_y()),
            Point::new(r.max_x(), r.max_y()),
            Point::new(r.min_x(), r.max_y()),
        ])
    }

    /// Rounded rectangle; `radius` is clamped to half the shorter side.
    pub fn rounded_rect(r: Rect, radius: f64) -> Path {
        let r = r.normalized();
        let rad = radius.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
        if rad <= f64::EPSILON {
            return Path::rect(r);
        }
        let k = rad * (1.0 - KAPPA);
        let (x0, y0, x1, y1) = (r.min_x(), r.min_y(), r.max_x(), r.max_y());
        let mut p = Path::new();
        p.move_to(Point::new(x0 + rad, y0))
            .line_to(Point::new(x1 - rad, y0))
            .cubic_to(Point::new(x1 - k, y0), Point::new(x1, y0 + k), Point::new(x1, y0 + rad))
            .line_to(Point::new(x1, y1 - rad))
            .cubic_to(Point::new(x1, y1 - k), Point::new(x1 - k, y1), Point::new(x1 - rad, y1))
            .line_to(Point::new(x0 + rad, y1))
            .cubic_to(Point::new(x0 + k, y1), Point::new(x0, y1 - k), Point::new(x0, y1 - rad))
            .line_to(Point::new(x0, y0 + rad))
            .cubic_to(Point::new(x0, y0 + k), Point::new(x0 + k, y0), Point::new(x0 + rad, y0))
            .close();
        p
    }

    /// Ellipse inscribed in `r` (four cubic arcs).
    pub fn ellipse(r: Rect) -> Path {
        let r = r.normalized();
        let (cx, cy) = (r.mid().x, r.mid().y);
        let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
        let (kx, ky) = (rx * KAPPA, ry * KAPPA);
        let mut p = Path::new();
        p.move_to(Point::new(cx + rx, cy))
            .cubic_to(Point::new(cx + rx, cy + ky), Point::new(cx + kx, cy + ry), Point::new(cx, cy + ry))
            .cubic_to(Point::new(cx - kx, cy + ry), Point::new(cx - rx, cy + ky), Point::new(cx - rx, cy))
            .cubic_to(Point::new(cx - rx, cy - ky), Point::new(cx - kx, cy - ry), Point::new(cx, cy - ry))
            .cubic_to(Point::new(cx + kx, cy - ry), Point::new(cx + rx, cy - ky), Point::new(cx + rx, cy))
            .close();
        p
    }

    /// Circle of `radius` around `c`.
    pub fn circle(c: Point, radius: f64) -> Path {
        Path::ellipse(Rect::new(c.x - radius, c.y - radius, radius * 2.0, radius * 2.0))
    }

    /// Bounds of all points including Bézier control points (a superset of
    /// the true bounds, which is what hit-slop and dirty rects need).
    pub fn control_bounds(&self) -> Option<Rect> {
        let mut pts = self.els.iter().flat_map(|e| match *e {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
            PathEl::QuadTo(c, p) => vec![c, p],
            PathEl::CubicTo(a, b, p) => vec![a, b, p],
            PathEl::Close => vec![],
        });
        let first = pts.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Flatten into polylines (one per sub-path; closed sub-paths repeat
    /// their first point at the end). Curves are subdivided into `steps`
    /// segments each.
    pub fn flatten(&self, steps: usize) -> Vec<Vec<Point>> {
        let steps = steps.max(1);
        let mut out: Vec<Vec<Point>> = Vec::new();
        let mut cur: Vec<Point> = Vec::new();
        let mut start = Point::ZERO;
        for el in &self.els {
            let last = cur.last().copied().unwrap_or(start);
            match *el {
                PathEl::MoveTo(p) => {
                    if cur.len() > 1 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    cur.push(p);
                    start = p;
                }
                PathEl::LineTo(p) => cur.push(p),
                PathEl::QuadTo(c, p) => {
                    for i in 1..=steps {
                        cur.push(quad_point(last, c, p, i as f64 / steps as f64));
                    }
                }
                PathEl::CubicTo(a, b, p) => {
                    for i in 1..=steps {
                        cur.push(cubic_point(last, a, b, p, i as f64 / steps as f64));
                    }
                }
                PathEl::Close => {
                    cur.push(start);
                    out.push(std::mem::take(&mut cur));
                }
            }
        }
        if cur.len() > 1 {
            out.push(cur);
        }
        out
    }

    /// Map every point (including control points) through `f`.
    pub fn map(&self, f: impl Fn(Point) -> Point) -> Path {
        Path {
            els: self
                .els
                .iter()
                .map(|e| match *e {
                    PathEl::MoveTo(p) => PathEl::MoveTo(f(p)),
                    PathEl::LineTo(p) => PathEl::LineTo(f(p)),
                    PathEl::QuadTo(c, p) => PathEl::QuadTo(f(c), f(p)),
                    PathEl::CubicTo(a, b, p) => PathEl::CubicTo(f(a), f(b), f(p)),
                    PathEl::Close => PathEl::Close,
                })
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Curves and distances
// ---------------------------------------------------------------------------

/// Point on the quadratic Bézier `a → b` with control `c` at `t`.
pub fn quad_point(a: Point, c: Point, b: Point, t: f64) -> Point {
    let u = 1.0 - t;
    Point::new(u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, u * u * a.y + 2.0 * u * t * c.y + t * t * b.y)
}

/// Point on the cubic Bézier `a → d` with controls `b`, `c` at `t`.
pub fn cubic_point(a: Point, b: Point, c: Point, d: Point, t: f64) -> Point {
    let u = 1.0 - t;
    let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Point::new(w0 * a.x + w1 * b.x + w2 * c.x + w3 * d.x, w0 * a.y + w1 * b.y + w2 * c.y + w3 * d.y)
}

/// Shortest distance from `p` to the segment `a–b`.
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= f64::EPSILON {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

/// Shortest distance from `p` to a polyline (∞ for an empty one).
pub fn distance_to_polyline(p: Point, pts: &[Point]) -> f64 {
    match pts {
        [] => f64::INFINITY,
        [only] => p.distance(*only),
        _ => pts.windows(2).map(|w| distance_to_segment(p, w[0], w[1])).fold(f64::INFINITY, f64::min),
    }
}

/// Even-odd point-in-polygon test.
pub fn point_in_polygon(p: Point, poly: &[Point]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Unit vector from `a` to `b`, or `None` if they coincide.
pub fn direction(a: Point, b: Point) -> Option<(f64, f64)> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    (len > 1e-9).then(|| (dx / len, dy / len))
}

/// Snap the vector `from → to` to the nearest multiple of 45°, keeping its
/// length (Shift while drawing lines and arrows).
pub fn snap_angle(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = dx.hypot(dy);
    if len <= f64::EPSILON {
        return to;
    }
    let step = std::f64::consts::FRAC_PI_4;
    let a = (dy.atan2(dx) / step).round() * step;
    Point::new(from.x + len * a.cos(), from.y + len * a.sin())
}

// ---------------------------------------------------------------------------
// Freehand
// ---------------------------------------------------------------------------

/// Ramer–Douglas–Peucker simplification. Endpoints are always kept.
pub fn simplify(points: &[Point], epsilon: f64) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((s, e)) = stack.pop() {
        let mut max_d = 0.0;
        let mut idx = s;
        for (i, p) in points.iter().enumerate().take(e).skip(s + 1) {
            let d = distance_to_segment(*p, points[s], points[e]);
            if d > max_d {
                max_d = d;
                idx = i;
            }
        }
        if max_d > epsilon {
            keep[idx] = true;
            stack.push((s, idx));
            stack.push((idx, e));
        }
    }
    points.iter().zip(keep).filter_map(|(p, k)| k.then_some(*p)).collect()
}

/// Smooth a freehand stroke: a Catmull-Rom spline through the points,
/// converted to cubic Béziers (so the curve passes through every sample).
pub fn smooth(points: &[Point]) -> Path {
    let mut path = Path::new();
    match points {
        [] => return path,
        [p] => {
            // A dot: a zero-length segment renders as a round cap.
            path.move_to(*p).line_to(*p);
            return path;
        }
        [a, b] => {
            path.move_to(*a).line_to(*b);
            return path;
        }
        _ => {}
    }
    path.move_to(points[0]);
    let n = points.len();
    for i in 0..n - 1 {
        let p0 = points[i.saturating_sub(1)];
        let p1 = points[i];
        let p2 = points[i + 1];
        let p3 = points[(i + 2).min(n - 1)];
        let c1 = Point::new(p1.x + (p2.x - p0.x) / 6.0, p1.y + (p2.y - p0.y) / 6.0);
        let c2 = Point::new(p2.x - (p3.x - p1.x) / 6.0, p2.y - (p3.y - p1.y) / 6.0);
        path.cubic_to(c1, c2, p2);
    }
    path
}

/// Append a freehand sample unless it is closer than `min_gap` to the last
/// one (pointer jitter).
pub fn push_sample(points: &mut Vec<Point>, p: Point, min_gap: f64) -> bool {
    match points.last() {
        Some(last) if last.distance(p) < min_gap => false,
        _ => {
            points.push(p);
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Arrows
// ---------------------------------------------------------------------------

/// Proportions of the arrow head relative to the stroke width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowHead {
    /// Tip to the back corners of the head.
    pub length: f64,
    /// Half the head's width at its back corners.
    pub half_width: f64,
    /// How far the shaft joins in front of the back corners (swallowtail),
    /// as a fraction of `length`.
    pub notch: f64,
}

impl ArrowHead {
    /// Head for a stroke `width`, shrunk for arrows shorter than ~2.5 heads
    /// so short arrows stay arrows.
    pub fn for_width(width: f64, arrow_length: f64) -> ArrowHead {
        let mut length = 3.6 * width + 9.0;
        length = length.min(arrow_length * 0.45).max(0.0);
        ArrowHead { length, half_width: length * 0.48, notch: 0.22 }
    }
}

/// Default bend of a curved arrow: the control point sits this fraction of
/// the arrow's length off its midpoint (the visible apex is half that).
pub const CURVE_BULGE: f64 = 0.25;

/// Number of samples used for curved shafts.
const CURVE_SAMPLES: usize = 32;

/// Centre line of an arrow (straight, or a quadratic curve through
/// `control`), sampled tail → tip.
pub fn arrow_centerline(start: Point, end: Point, control: Option<Point>) -> Vec<Point> {
    match control {
        None => vec![start, end],
        Some(c) => (0..=CURVE_SAMPLES).map(|i| quad_point(start, c, end, i as f64 / CURVE_SAMPLES as f64)).collect(),
    }
}

/// Cumulative lengths along a polyline (`[0, |p0p1|, …]`).
pub fn polyline_lengths(pts: &[Point]) -> Vec<f64> {
    let mut acc = 0.0;
    let mut out = Vec::with_capacity(pts.len());
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            acc += pts[i - 1].distance(*p);
        }
        out.push(acc);
    }
    out
}

/// The point `d` along a polyline (clamped to its ends). `lengths` is
/// [`polyline_lengths`] of `pts`.
pub fn point_along(pts: &[Point], lengths: &[f64], d: f64) -> Point {
    let Some(first) = pts.first() else { return Point::ZERO };
    if d <= 0.0 {
        return *first;
    }
    for i in 1..pts.len() {
        if lengths[i] >= d {
            let seg = lengths[i] - lengths[i - 1];
            let t = if seg > 0.0 { (d - lengths[i - 1]) / seg } else { 0.0 };
            return pts[i - 1].lerp(pts[i], t.clamp(0.0, 1.0));
        }
    }
    *pts.last().unwrap_or(first)
}

/// The part of a polyline between distances `from` and `to` along it.
pub fn sub_polyline(pts: &[Point], from: f64, to: f64) -> Vec<Point> {
    let lengths = polyline_lengths(pts);
    let total = *lengths.last().unwrap_or(&0.0);
    let (from, to) = (from.clamp(0.0, total), to.clamp(0.0, total));
    if to <= from {
        return Vec::new();
    }
    let mut out = vec![point_along(pts, &lengths, from)];
    out.extend(pts.iter().zip(&lengths).filter(|(_, l)| **l > from && **l < to).map(|(p, _)| *p));
    out.push(point_along(pts, &lengths, to));
    out
}

/// The corners of an arrowhead whose tip is at `tip`, pointing along the
/// unit vector `dir`: `[left wing, tip, right wing, notch]` (the notch is
/// where a filled swallowtail head meets the shaft).
pub fn head_points(tip: Point, dir: (f64, f64), head: ArrowHead) -> [Point; 4] {
    let (dx, dy) = dir;
    let (nx, ny) = (-dy, dx);
    let back = Point::new(tip.x - dx * head.length, tip.y - dy * head.length);
    let notch_at = head.length * (1.0 - head.notch);
    [
        Point::new(back.x + nx * head.half_width, back.y + ny * head.half_width),
        tip,
        Point::new(back.x - nx * head.half_width, back.y - ny * head.half_width),
        Point::new(tip.x - dx * notch_at, tip.y - dy * notch_at),
    ]
}

/// The filled outline of a tapered arrow: the shaft grows from 40 % of
/// `width` at the tail to the full width where it meets a swallowtail head.
/// Returns an empty path for a zero-length arrow.
pub fn arrow_outline(start: Point, end: Point, control: Option<Point>, width: f64) -> Path {
    let line = arrow_centerline(start, end, control);
    let lengths: Vec<f64> = std::iter::once(0.0)
        .chain(line.windows(2).scan(0.0, |acc, w| {
            *acc += w[0].distance(w[1]);
            Some(*acc)
        }))
        .collect();
    let total = *lengths.last().unwrap_or(&0.0);
    if total <= 1e-6 {
        return Path::new();
    }
    let head = ArrowHead::for_width(width, total);
    // The shaft ends where the head's notch is.
    let shaft_end = (total - head.length * (1.0 - head.notch)).max(0.0);
    let mut shaft: Vec<(Point, f64)> = Vec::new();
    for (i, p) in line.iter().enumerate() {
        if lengths[i] >= shaft_end {
            let prev = if i == 0 { *p } else { line[i - 1] };
            let seg = lengths[i] - if i == 0 { 0.0 } else { lengths[i - 1] };
            let t = if seg > 0.0 { (shaft_end - (lengths[i] - seg)) / seg } else { 0.0 };
            shaft.push((prev.lerp(*p, t.clamp(0.0, 1.0)), shaft_end));
            break;
        }
        shaft.push((*p, lengths[i]));
    }
    if shaft.len() < 2 {
        shaft.insert(0, (start, 0.0));
    }
    // Aim the head along the last part of the visible shaft.
    let aim_from = shaft.iter().rev().find(|(p, _)| p.distance(end) > head.length * 0.5).map(|s| s.0).unwrap_or(start);
    let Some((dx, dy)) = direction(aim_from, end).or_else(|| direction(start, end)) else { return Path::new() };
    let (nx, ny) = (-dy, dx);
    let half = |at: f64| {
        let f = if shaft_end > 0.0 { (at / shaft_end).clamp(0.0, 1.0) } else { 1.0 };
        width * 0.5 * (0.4 + 0.6 * f)
    };
    let normal_at = |i: usize| -> (f64, f64) {
        let a = shaft[i.saturating_sub(1)].0;
        let b = shaft[(i + 1).min(shaft.len() - 1)].0;
        direction(a, b).map(|(x, y)| (-y, x)).unwrap_or((nx, ny))
    };
    let mut left = Vec::with_capacity(shaft.len());
    let mut right = Vec::with_capacity(shaft.len());
    for (i, (p, at)) in shaft.iter().enumerate() {
        let (ux, uy) = if i + 1 == shaft.len() { (nx, ny) } else { normal_at(i) };
        let h = half(*at);
        left.push(Point::new(p.x + ux * h, p.y + uy * h));
        right.push(Point::new(p.x - ux * h, p.y - uy * h));
    }
    let back = Point::new(end.x - dx * head.length, end.y - dy * head.length);
    let wing_l = Point::new(back.x + nx * head.half_width, back.y + ny * head.half_width);
    let wing_r = Point::new(back.x - nx * head.half_width, back.y - ny * head.half_width);
    let mut poly = left;
    poly.push(wing_l);
    poly.push(end);
    poly.push(wing_r);
    poly.extend(right.into_iter().rev());
    Path::polygon(&poly)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn polyline_walking_and_trimming() {
        let pts = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)];
        let l = polyline_lengths(&pts);
        assert_eq!(l, vec![0.0, 10.0, 20.0]);
        assert_eq!(point_along(&pts, &l, 15.0), p(10.0, 5.0));
        assert_eq!(point_along(&pts, &l, -1.0), p(0.0, 0.0));
        assert_eq!(point_along(&pts, &l, 99.0), p(10.0, 10.0));
        assert_eq!(sub_polyline(&pts, 5.0, 15.0), vec![p(5.0, 0.0), p(10.0, 0.0), p(10.0, 5.0)]);
        assert!(sub_polyline(&pts, 12.0, 12.0).is_empty());
        assert_eq!(sub_polyline(&pts, -5.0, 100.0), vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)]);
    }

    #[test]
    fn head_points_are_symmetric_behind_the_tip() {
        let head = ArrowHead { length: 10.0, half_width: 4.0, notch: 0.2 };
        let [l, tip, r, notch] = head_points(p(100.0, 0.0), (1.0, 0.0), head);
        assert_eq!(tip, p(100.0, 0.0));
        assert_eq!(l, p(90.0, 4.0));
        assert_eq!(r, p(90.0, -4.0));
        assert_eq!(notch, p(92.0, 0.0));
    }

    #[test]
    fn segment_distance() {
        assert_eq!(distance_to_segment(p(5.0, 3.0), p(0.0, 0.0), p(10.0, 0.0)), 3.0);
        assert_eq!(distance_to_segment(p(-3.0, 4.0), p(0.0, 0.0), p(10.0, 0.0)), 5.0);
        assert_eq!(distance_to_segment(p(1.0, 1.0), p(0.0, 0.0), p(0.0, 0.0)), 2f64.sqrt());
        assert_eq!(distance_to_polyline(p(0.0, 0.0), &[]), f64::INFINITY);
    }

    #[test]
    fn polygon_containment() {
        let sq = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        assert!(point_in_polygon(p(5.0, 5.0), &sq));
        assert!(!point_in_polygon(p(15.0, 5.0), &sq));
        assert!(!point_in_polygon(p(5.0, 5.0), &sq[..2]));
    }

    #[test]
    fn beziers_hit_endpoints_and_midpoints() {
        let (a, c, b) = (p(0.0, 0.0), p(5.0, 10.0), p(10.0, 0.0));
        assert_eq!(quad_point(a, c, b, 0.0), a);
        assert_eq!(quad_point(a, c, b, 1.0), b);
        assert_eq!(quad_point(a, c, b, 0.5), p(5.0, 5.0));
        let d = p(10.0, 10.0);
        assert_eq!(cubic_point(a, c, b, d, 1.0), d);
    }

    #[test]
    fn ellipse_flattens_onto_its_boundary() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        let flat = Path::ellipse(r).flatten(16);
        assert_eq!(flat.len(), 1);
        for q in &flat[0] {
            let nx = (q.x - 60.0) / 50.0;
            let ny = (q.y - 45.0) / 25.0;
            // The 4-arc cubic approximation is within 0.03 % of a true ellipse.
            assert!((nx.hypot(ny) - 1.0).abs() < 1e-3, "{q:?}");
        }
        let b = Path::ellipse(r).control_bounds().unwrap();
        assert_eq!(b, r);
    }

    #[test]
    fn rounded_rect_clamps_radius() {
        let r = Rect::new(0.0, 0.0, 20.0, 10.0);
        let path = Path::rounded_rect(r, 100.0);
        let b = path.control_bounds().unwrap();
        assert_eq!(b, r);
        assert_eq!(Path::rounded_rect(r, 0.0), Path::rect(r));
    }

    #[test]
    fn simplify_drops_collinear_points_and_keeps_corners() {
        let pts: Vec<Point> = (0..=10).map(|i| p(i as f64, 0.0)).chain((1..=10).map(|i| p(10.0, i as f64))).collect();
        let s = simplify(&pts, 0.1);
        assert_eq!(s, vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)]);
        assert_eq!(simplify(&pts[..2], 1.0).len(), 2);
    }

    #[test]
    fn smoothing_passes_through_samples() {
        let pts = [p(0.0, 0.0), p(10.0, 5.0), p(20.0, 0.0), p(30.0, 5.0)];
        let path = smooth(&pts);
        assert_eq!(path.els.len(), 4);
        let ends: Vec<Point> = path
            .els
            .iter()
            .map(|e| match *e {
                PathEl::MoveTo(p) | PathEl::CubicTo(_, _, p) => p,
                _ => panic!("unexpected element"),
            })
            .collect();
        assert_eq!(ends, pts.to_vec());
        assert_eq!(smooth(&[]).els.len(), 0);
        assert_eq!(smooth(&pts[..1]).els.len(), 2, "a dot is a zero-length line");
    }

    #[test]
    fn jitter_is_filtered() {
        let mut v = vec![p(0.0, 0.0)];
        assert!(!push_sample(&mut v, p(0.5, 0.5), 1.5));
        assert!(push_sample(&mut v, p(2.0, 0.0), 1.5));
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn angle_snapping() {
        let s = snap_angle(p(0.0, 0.0), p(10.0, 1.0));
        assert!((s.y).abs() < 1e-9 && (s.x - 101f64.sqrt()).abs() < 1e-9);
        let d = snap_angle(p(0.0, 0.0), p(10.0, 9.0));
        assert!((d.x - d.y).abs() < 1e-9);
    }

    #[test]
    fn straight_arrow_outline_is_symmetric_and_tipped() {
        let path = arrow_outline(p(0.0, 0.0), p(100.0, 0.0), None, 4.0);
        let poly = &path.flatten(1)[0];
        // Tip is the rightmost point.
        let max_x = poly.iter().map(|q| q.x).fold(f64::MIN, f64::max);
        assert_eq!(max_x, 100.0);
        // Symmetric about the x axis (the last point repeats the first).
        let ys: f64 = poly[..poly.len() - 1].iter().map(|q| q.y).sum();
        assert!(ys.abs() < 1e-9);
        // Head is wider than the shaft; tail is narrower than the shaft end.
        let head = ArrowHead::for_width(4.0, 100.0);
        let max_y = poly.iter().map(|q| q.y).fold(f64::MIN, f64::max);
        assert!((max_y - head.half_width).abs() < 1e-9);
        assert!((poly[0].y - 0.8).abs() < 1e-9, "tail half-width is 40 % of 2");
        assert!(point_in_polygon(p(98.0, 0.0), &poly[..poly.len() - 1]));
        assert!(point_in_polygon(p(5.0, 0.0), &poly[..poly.len() - 1]));
        assert!(!point_in_polygon(p(5.0, 3.0), &poly[..poly.len() - 1]));
    }

    #[test]
    fn short_arrows_get_smaller_heads_and_zero_length_is_empty() {
        let short = ArrowHead::for_width(8.0, 20.0);
        assert!(short.length <= 9.0 + 1e-9);
        assert!(arrow_outline(p(1.0, 1.0), p(1.0, 1.0), None, 4.0).is_empty());
    }

    #[test]
    fn curved_arrow_follows_its_curve() {
        let (a, c, b) = (p(0.0, 0.0), p(50.0, 60.0), p(100.0, 0.0));
        let path = arrow_outline(a, b, Some(c), 4.0);
        let poly = &path.flatten(1)[0];
        // The apex of the curve (t = 0.5 → y = 30) lies inside the outline.
        assert!(point_in_polygon(p(50.0, 30.0), &poly[..poly.len() - 1]));
        assert!(!point_in_polygon(p(50.0, 0.0), &poly[..poly.len() - 1]));
        assert!(poly.contains(&b), "tip is part of the outline");
    }

    #[test]
    fn map_and_extend() {
        let mut a = Path::rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        let b = a.map(|q| p(q.x * 2.0, q.y * 2.0));
        assert_eq!(b.control_bounds().unwrap(), Rect::new(0.0, 0.0, 2.0, 2.0));
        a.extend(&b);
        assert_eq!(a.flatten(1).len(), 2);
    }
}
