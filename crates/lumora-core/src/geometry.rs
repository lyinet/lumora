//! Small, dependency-free 2D geometry types.
//!
//! These intentionally mirror `CGPoint`/`CGSize`/`CGRect` (all `f64`) so the
//! conversion at the FFI boundary is trivial, but they carry the behaviour the
//! rest of Lumora needs (normalisation, clamping, pixel rounding, handles).

use std::ops::{Add, Sub};

/// A point in 2D space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance to `other`.
    #[inline]
    pub fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    /// Linear interpolation towards `other` (`t = 0` → self, `t = 1` → other).
    #[inline]
    pub fn lerp(self, other: Point, t: f64) -> Point {
        Point::new(self.x + (other.x - self.x) * t, self.y + (other.y - self.y) * t)
    }

    /// Floor both coordinates onto the device pixel grid for `scale`.
    #[inline]
    pub fn floor_to_pixel(self, scale: f64) -> Point {
        Point::new((self.x * scale).floor() / scale, (self.y * scale).floor() / scale)
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, rhs: Point) -> Point {
        Point::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, rhs: Point) -> Point {
        Point::new(self.x - rhs.x, self.y - rhs.y)
    }
}

/// A width/height pair.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const ZERO: Size = Size { width: 0.0, height: 0.0 };

    #[inline]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    /// Width divided by height, or `None` for a degenerate size.
    pub fn aspect_ratio(self) -> Option<f64> {
        (self.height.abs() > f64::EPSILON).then(|| self.width / self.height)
    }

    /// Scale uniformly so the size fits inside `bounds`, preserving aspect.
    pub fn fit_within(self, bounds: Size) -> Size {
        if self.width <= 0.0 || self.height <= 0.0 {
            return Size::ZERO;
        }
        let s = (bounds.width / self.width).min(bounds.height / self.height);
        Size::new(self.width * s, self.height * s)
    }
}

/// An axis-aligned rectangle. `origin` is the minimum corner once normalised.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const ZERO: Rect = Rect { origin: Point::ZERO, size: Size::ZERO };

    #[inline]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { origin: Point { x, y }, size: Size { width, height } }
    }

    /// The (normalised) rectangle spanned by two arbitrary corners.
    pub fn from_corners(a: Point, b: Point) -> Rect {
        let min_x = a.x.min(b.x);
        let min_y = a.y.min(b.y);
        Rect::new(min_x, min_y, (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    /// Ensure width and height are non-negative.
    pub fn normalized(self) -> Rect {
        Rect::from_corners(self.origin, Point::new(self.max_x(), self.max_y()))
    }

    #[inline]
    pub fn min_x(&self) -> f64 {
        self.origin.x
    }
    #[inline]
    pub fn min_y(&self) -> f64 {
        self.origin.y
    }
    #[inline]
    pub fn max_x(&self) -> f64 {
        self.origin.x + self.size.width
    }
    #[inline]
    pub fn max_y(&self) -> f64 {
        self.origin.y + self.size.height
    }
    #[inline]
    pub fn width(&self) -> f64 {
        self.size.width
    }
    #[inline]
    pub fn height(&self) -> f64 {
        self.size.height
    }
    #[inline]
    pub fn mid(&self) -> Point {
        Point::new(self.origin.x + self.size.width / 2.0, self.origin.y + self.size.height / 2.0)
    }

    pub fn area(&self) -> f64 {
        self.size.width.max(0.0) * self.size.height.max(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.size.width <= 0.0 || self.size.height <= 0.0
    }

    /// Half-open containment: the min edges are inside, max edges are not.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.min_x() && p.x < self.max_x() && p.y >= self.min_y() && p.y < self.max_y()
    }

    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.min_x() >= self.min_x()
            && other.min_y() >= self.min_y()
            && other.max_x() <= self.max_x()
            && other.max_y() <= self.max_y()
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.min_x() < other.max_x()
            && other.min_x() < self.max_x()
            && self.min_y() < other.max_y()
            && other.min_y() < self.max_y()
    }

    /// The overlapping region, or `None` if the rectangles don't overlap.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.min_x().max(other.min_x());
        let y0 = self.min_y().max(other.min_y());
        let x1 = self.max_x().min(other.max_x());
        let y1 = self.max_y().min(other.max_y());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Smallest rectangle containing both.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        Rect::from_corners(
            Point::new(self.min_x().min(other.min_x()), self.min_y().min(other.min_y())),
            Point::new(self.max_x().max(other.max_x()), self.max_y().max(other.max_y())),
        )
    }

    pub fn offset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.origin.x + dx, self.origin.y + dy, self.size.width, self.size.height)
    }

    /// Shrink (positive) or grow (negative) on every side.
    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(
            self.origin.x + dx,
            self.origin.y + dy,
            (self.size.width - 2.0 * dx).max(0.0),
            (self.size.height - 2.0 * dy).max(0.0),
        )
    }

    /// Translate (without resizing) so the rect lies inside `bounds`. If the
    /// rect is larger than `bounds` on an axis it is pinned to the min edge.
    pub fn translated_within(&self, bounds: &Rect) -> Rect {
        let mut r = *self;
        if r.max_x() > bounds.max_x() {
            r.origin.x = bounds.max_x() - r.width();
        }
        if r.max_y() > bounds.max_y() {
            r.origin.y = bounds.max_y() - r.height();
        }
        r.origin.x = r.origin.x.max(bounds.min_x());
        r.origin.y = r.origin.y.max(bounds.min_y());
        r
    }

    /// Clamp a point into the rect (max edges inclusive).
    pub fn clamp_point(&self, p: Point) -> Point {
        Point::new(p.x.clamp(self.min_x(), self.max_x()), p.y.clamp(self.min_y(), self.max_y()))
    }

    /// Expand outward so every edge lands on the device-pixel grid for
    /// `scale`. Captures must be pixel aligned or the result looks blurry.
    pub fn integral(&self, scale: f64) -> Rect {
        let x0 = (self.min_x() * scale).floor() / scale;
        let y0 = (self.min_y() * scale).floor() / scale;
        let x1 = (self.max_x() * scale).ceil() / scale;
        let y1 = (self.max_y() * scale).ceil() / scale;
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Round every edge to the nearest device pixel (used for display
    /// geometry where growing would look wrong).
    pub fn rounded(&self, scale: f64) -> Rect {
        let x0 = (self.min_x() * scale).round() / scale;
        let y0 = (self.min_y() * scale).round() / scale;
        let x1 = (self.max_x() * scale).round() / scale;
        let y1 = (self.max_y() * scale).round() / scale;
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Size in device pixels.
    pub fn pixel_size(&self, scale: f64) -> (u32, u32) {
        ((self.width() * scale).round().max(0.0) as u32, (self.height() * scale).round().max(0.0) as u32)
    }

    /// Interpolate each component.
    pub fn lerp(&self, other: &Rect, t: f64) -> Rect {
        let o = self.origin.lerp(other.origin, t);
        Rect::new(
            o.x,
            o.y,
            self.width() + (other.width() - self.width()) * t,
            self.height() + (other.height() - self.height()) * t,
        )
    }

    /// Express this rect relative to `container`'s origin (e.g. global →
    /// display-local).
    pub fn relative_to(&self, container: &Rect) -> Rect {
        self.offset(-container.min_x(), -container.min_y())
    }

    /// Location of a resize handle on this rect.
    pub fn handle_point(&self, h: Handle) -> Point {
        let (fx, fy) = h.fractions();
        Point::new(self.min_x() + self.width() * fx, self.min_y() + self.height() * fy)
    }

    /// The handle under `p`, if any, using a square hit area of `radius`.
    /// Corners win over edges so small selections remain resizable.
    pub fn handle_at(&self, p: Point, radius: f64) -> Option<Handle> {
        Handle::ALL
            .iter()
            .copied()
            .filter(|h| {
                let c = self.handle_point(*h);
                (p.x - c.x).abs() <= radius && (p.y - c.y).abs() <= radius
            })
            .min_by(|a, b| {
                let da = self.handle_point(*a).distance(p) - if a.is_corner() { radius } else { 0.0 };
                let db = self.handle_point(*b).distance(p) - if b.is_corner() { radius } else { 0.0 };
                da.total_cmp(&db)
            })
    }
}

/// One of the eight resize handles of a selection rectangle.
///
/// Names are in y-down space: `Top` is the `min_y` edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    pub const CORNERS: [Handle; 4] = [Handle::TopLeft, Handle::TopRight, Handle::BottomRight, Handle::BottomLeft];

    /// Position of the handle as fractions of the rect's width/height.
    pub fn fractions(self) -> (f64, f64) {
        match self {
            Handle::TopLeft => (0.0, 0.0),
            Handle::Top => (0.5, 0.0),
            Handle::TopRight => (1.0, 0.0),
            Handle::Right => (1.0, 0.5),
            Handle::BottomRight => (1.0, 1.0),
            Handle::Bottom => (0.5, 1.0),
            Handle::BottomLeft => (0.0, 1.0),
            Handle::Left => (0.0, 0.5),
        }
    }

    pub fn is_corner(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft)
    }

    /// The handle diagonally/directly opposite; it stays fixed while resizing.
    pub fn opposite(self) -> Handle {
        match self {
            Handle::TopLeft => Handle::BottomRight,
            Handle::Top => Handle::Bottom,
            Handle::TopRight => Handle::BottomLeft,
            Handle::Right => Handle::Left,
            Handle::BottomRight => Handle::TopLeft,
            Handle::Bottom => Handle::Top,
            Handle::BottomLeft => Handle::TopRight,
            Handle::Left => Handle::Right,
        }
    }

    /// Whether dragging this handle changes the horizontal / vertical extent.
    pub fn axes(self) -> (bool, bool) {
        match self {
            Handle::Top | Handle::Bottom => (false, true),
            Handle::Left | Handle::Right => (true, false),
            _ => (true, true),
        }
    }
}

/// Convert a rect between bottom-left-origin (AppKit) and top-left-origin
/// (Quartz) global coordinates. The operation is its own inverse.
/// `primary_height` is the height of the primary display (the one whose
/// AppKit frame origin is `(0, 0)`).
pub fn flip_y(r: Rect, primary_height: f64) -> Rect {
    Rect::new(r.min_x(), primary_height - r.max_y(), r.width(), r.height())
}

/// Point version of [`flip_y`].
pub fn flip_point_y(p: Point, primary_height: f64) -> Point {
    Point::new(p.x, primary_height - p.y)
}

/// A global AppKit point (bottom-left origin, y up) in the top-down local
/// space of an overlay that covers `frame` (its AppKit global frame): x
/// from the frame's left edge, y from its **top** edge. This is what a
/// y-down (flipped) layer tree inside that overlay expects.
pub fn appkit_to_local_top_down(p: Point, frame: Rect) -> Point {
    Point::new(p.x - frame.min_x(), frame.max_y() - p.y)
}

/// Rect version of [`appkit_to_local_top_down`]: the rect's top edge (its
/// AppKit `max_y`) becomes its local `min_y`.
pub fn appkit_rect_to_local_top_down(r: Rect, frame: Rect) -> Rect {
    Rect::new(r.min_x() - frame.min_x(), frame.max_y() - r.max_y(), r.width(), r.height())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appkit_point_to_overlay_local_top_down() {
        // Primary 1512×982 at the origin.
        let primary = Rect::new(0.0, 0.0, 1512.0, 982.0);
        // A click near the bottom-right (the stop button) stays bottom-right.
        let p = appkit_to_local_top_down(Point::new(1400.0, 40.0), primary);
        assert_eq!(p, Point::new(1400.0, 942.0));
        // Top-left corner of the screen is local (0, 0).
        assert_eq!(appkit_to_local_top_down(Point::new(0.0, 982.0), primary), Point::new(0.0, 0.0));
        // A screen to the left and higher up (negative x, positive y).
        let left = Rect::new(-2560.0, 200.0, 2560.0, 1440.0);
        assert_eq!(appkit_to_local_top_down(Point::new(-2560.0, 1640.0), left), Point::new(0.0, 0.0));
        assert_eq!(appkit_to_local_top_down(Point::new(-10.0, 210.0), left), Point::new(2550.0, 1430.0));
        // A screen below the primary (negative y).
        let below = Rect::new(300.0, -1080.0, 1920.0, 1080.0);
        assert_eq!(appkit_to_local_top_down(Point::new(400.0, -1000.0), below), Point::new(100.0, 1000.0));
        // y-mirrored clicks must map to different local points (the v0.6.1
        // bug drew `h - y` instead of `y`).
        let a = appkit_to_local_top_down(Point::new(10.0, 100.0), primary);
        let b = appkit_to_local_top_down(Point::new(10.0, 882.0), primary);
        assert!(a.y > b.y);
        assert!((a.y + b.y - primary.height()).abs() < 1e-9);
    }

    #[test]
    fn appkit_rect_to_overlay_local_top_down() {
        let screen = Rect::new(-1920.0, -300.0, 1920.0, 1200.0);
        // A rect touching the screen's top-left corner.
        let r = Rect::new(-1920.0, 700.0, 300.0, 200.0);
        assert_eq!(appkit_rect_to_local_top_down(r, screen), Rect::new(0.0, 0.0, 300.0, 200.0));
        // Its bottom-left corner point agrees with the point helper.
        let bl = appkit_to_local_top_down(Point::new(r.min_x(), r.min_y()), screen);
        assert_eq!(bl, Point::new(0.0, 200.0));
    }

    #[test]
    fn from_corners_normalises() {
        let r = Rect::from_corners(Point::new(10.0, 20.0), Point::new(2.0, 5.0));
        assert_eq!(r, Rect::new(2.0, 5.0, 8.0, 15.0));
        assert_eq!(Rect::new(10.0, 10.0, -4.0, -6.0).normalized(), Rect::new(6.0, 4.0, 4.0, 6.0));
    }

    #[test]
    fn containment_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(!r.contains(Point::new(10.0, 5.0)));
        assert!(r.contains_rect(&Rect::new(1.0, 1.0, 9.0, 9.0)));
        assert!(!r.contains_rect(&Rect::new(1.0, 1.0, 10.0, 9.0)));
    }

    #[test]
    fn intersection_and_union() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersection(&b), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        assert_eq!(a.intersection(&Rect::new(10.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(Rect::ZERO.union(&b), b);
    }

    #[test]
    fn translated_within_keeps_size() {
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        let r = Rect::new(90.0, -5.0, 20.0, 20.0).translated_within(&bounds);
        assert_eq!(r, Rect::new(80.0, 0.0, 20.0, 20.0));
        let huge = Rect::new(-10.0, -10.0, 200.0, 50.0).translated_within(&bounds);
        assert_eq!(huge.origin, Point::new(0.0, 0.0));
        assert_eq!(huge.size, Size::new(200.0, 50.0));
    }

    #[test]
    fn integral_expands_to_pixel_grid() {
        let r = Rect::new(10.3, 10.6, 5.1, 5.1).integral(2.0);
        assert_eq!(r, Rect::new(10.0, 10.5, 5.5, 5.5));
        assert_eq!(r.pixel_size(2.0), (11, 11));
        let rounded = Rect::new(10.3, 10.6, 5.1, 5.1).rounded(1.0);
        assert_eq!(rounded, Rect::new(10.0, 11.0, 5.0, 5.0));
    }

    #[test]
    fn flip_is_involution() {
        let r = Rect::new(10.0, 20.0, 30.0, 40.0);
        let f = flip_y(r, 900.0);
        assert_eq!(f, Rect::new(10.0, 840.0, 30.0, 40.0));
        assert_eq!(flip_y(f, 900.0), r);
        assert_eq!(flip_point_y(Point::new(1.0, 100.0), 900.0), Point::new(1.0, 800.0));
    }

    #[test]
    fn handles_hit_test_prefers_corners() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(r.handle_at(Point::new(1.0, 1.0), 6.0), Some(Handle::TopLeft));
        assert_eq!(r.handle_at(Point::new(50.0, 49.0), 6.0), Some(Handle::Bottom));
        assert_eq!(r.handle_at(Point::new(50.0, 25.0), 6.0), None);
        // A tiny rect: corner and edge handles overlap; corner wins.
        let tiny = Rect::new(0.0, 0.0, 6.0, 6.0);
        assert!(tiny.handle_at(Point::new(6.0, 6.0), 6.0).unwrap().is_corner());
        for h in Handle::ALL {
            assert_eq!(h.opposite().opposite(), h);
        }
    }

    #[test]
    fn size_helpers() {
        assert_eq!(Size::new(400.0, 200.0).fit_within(Size::new(100.0, 100.0)), Size::new(100.0, 50.0));
        assert_eq!(Size::new(0.0, 10.0).fit_within(Size::new(10.0, 10.0)), Size::ZERO);
        assert_eq!(Size::new(16.0, 9.0).aspect_ratio().map(|a| (a * 9.0).round()), Some(16.0));
        assert_eq!(Size::new(1.0, 0.0).aspect_ratio(), None);
    }

    #[test]
    fn lerp_rect() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(10.0, 20.0, 30.0, 50.0);
        assert_eq!(a.lerp(&b, 0.5), Rect::new(5.0, 10.0, 20.0, 30.0));
        assert_eq!(b.relative_to(&Rect::new(10.0, 10.0, 1.0, 1.0)).origin, Point::new(0.0, 10.0));
    }
}
