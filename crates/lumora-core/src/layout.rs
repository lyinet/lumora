//! Placement math for HUDs: Quick Access stack, size pill, magnifier loupe
//! and the shutter "fly into card" transform.
//!
//! Functions say which coordinate space they expect. Quick Access works in
//! AppKit screen space (bottom-left origin, y up) because it positions
//! ordinary windows; overlay helpers work in the overlay view's flipped
//! (y-down) space.

use crate::geometry::{Point, Rect, Size};

/// Maximum number of Quick Access cards visible at once.
pub const MAX_CARDS: usize = 3;

/// Frames for a stack of `count` Quick Access cards anchored bottom-left of
/// `visible` (AppKit space: the screen's `visibleFrame`). Index 0 is the
/// newest card and sits at the bottom; older cards stack upward.
pub fn quick_access_frames(visible: Rect, card: Size, count: usize, margin: f64, spacing: f64) -> Vec<Rect> {
    (0..count.min(MAX_CARDS))
        .map(|i| {
            Rect::new(
                visible.min_x() + margin,
                visible.min_y() + margin + i as f64 * (card.height + spacing),
                card.width,
                card.height,
            )
        })
        .collect()
}

/// Like [`quick_access_frames`] but for cards of differing heights (each
/// card's height follows its image's aspect). Index 0 is the newest, at the
/// bottom.
pub fn stack_frames(visible: Rect, sizes: &[Size], margin: f64, spacing: f64) -> Vec<Rect> {
    let mut y = visible.min_y() + margin;
    sizes
        .iter()
        .take(MAX_CARDS)
        .map(|s| {
            let r = Rect::new(visible.min_x() + margin, y, s.width, s.height);
            y += s.height + spacing;
            r
        })
        .collect()
}

/// Where a card starts before sliding in: just off the left edge of the
/// screen at its final height.
pub fn offscreen_left(frame: Rect, visible: Rect) -> Rect {
    Rect { origin: Point::new(visible.min_x() - frame.width() - 24.0, frame.min_y()), size: frame.size }
}

/// Size of a card for an image of `image` aspect: fixed width, height
/// following the aspect but clamped so panoramas and tall shots stay sane.
pub fn card_size(image: Size, width: f64, min_h: f64, max_h: f64) -> Size {
    let h = match image.aspect_ratio() {
        Some(a) if a > 0.0 => width / a,
        _ => width * 0.625,
    };
    Size::new(width, h.clamp(min_h, max_h))
}

/// Position the dimension pill (`pill` size) relative to `selection` within
/// `screen` (y-down). Preference: centred below; else above; else inside the
/// selection's bottom edge. Always clamped on-screen.
pub fn pill_frame(selection: Rect, pill: Size, screen: Rect, gap: f64) -> Rect {
    let x = selection.mid().x - pill.width / 2.0;
    let below = selection.max_y() + gap;
    let above = selection.min_y() - gap - pill.height;
    let y = if below + pill.height <= screen.max_y() {
        below
    } else if above >= screen.min_y() {
        above
    } else {
        selection.max_y() - gap - pill.height
    };
    Rect::new(x, y, pill.width, pill.height).translated_within(&screen)
}

/// Position the loupe near `cursor` (y-down). It sits below-right of the
/// cursor and flips to the other side on an axis when it would leave
/// `screen`.
pub fn loupe_frame(cursor: Point, loupe: Size, screen: Rect, offset: f64) -> Rect {
    let mut x = cursor.x + offset;
    let mut y = cursor.y + offset;
    if x + loupe.width > screen.max_x() {
        x = cursor.x - offset - loupe.width;
    }
    if y + loupe.height > screen.max_y() {
        y = cursor.y - offset - loupe.height;
    }
    Rect::new(x, y, loupe.width, loupe.height).translated_within(&screen)
}

/// A pixel-space rectangle `(x, y, w, h)` in an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The `grid`×`grid` block of pixels centred on `(cx, cy)` to show in the
/// loupe, shifted (not shrunk) to stay inside a `width`×`height` image.
/// `grid` should be odd so a centre pixel exists.
pub fn loupe_sample(cx: i64, cy: i64, grid: u32, width: u32, height: u32) -> Option<PixelRect> {
    if width == 0 || height == 0 || grid == 0 {
        return None;
    }
    let g = grid.min(width).min(height);
    let half = i64::from(g / 2);
    let x = (cx - half).clamp(0, i64::from(width - g)) as u32;
    let y = (cy - half).clamp(0, i64::from(height - g)) as u32;
    Some(PixelRect { x, y, w: g, h: g })
}

/// The affine (uniform scale + translation of the centre) that maps `from`
/// onto `to`, aspect-fit. Used for the shutter animation where the captured
/// region shrinks and flies into the Quick Access card.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlyTransform {
    pub scale: f64,
    pub dx: f64,
    pub dy: f64,
}

pub fn fly_transform(from: Rect, to: Rect) -> FlyTransform {
    let scale = if from.is_empty() { 1.0 } else { (to.width() / from.width()).min(to.height() / from.height()) };
    let d = to.mid() - from.mid();
    FlyTransform { scale, dx: d.x, dy: d.y }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_grows_upward_and_caps() {
        let vis = Rect::new(0.0, 25.0, 1440.0, 850.0);
        let f = quick_access_frames(vis, Size::new(240.0, 150.0), 5, 20.0, 12.0);
        assert_eq!(f.len(), MAX_CARDS);
        assert_eq!(f[0], Rect::new(20.0, 45.0, 240.0, 150.0));
        assert_eq!(f[1].min_y(), 45.0 + 162.0);
        assert!(f[2].min_y() > f[1].min_y());
        let off = offscreen_left(f[0], vis);
        assert!(off.max_x() < vis.min_x());
        assert_eq!(off.min_y(), f[0].min_y());
    }

    #[test]
    fn variable_height_stack() {
        let vis = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let sizes = [Size::new(200.0, 100.0), Size::new(200.0, 150.0), Size::new(200.0, 120.0), Size::new(1.0, 1.0)];
        let f = stack_frames(vis, &sizes, 10.0, 8.0);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], Rect::new(10.0, 10.0, 200.0, 100.0));
        assert_eq!(f[1].min_y(), 118.0);
        assert_eq!(f[2].min_y(), 276.0);
    }

    #[test]
    fn card_sizes() {
        assert_eq!(card_size(Size::new(1600.0, 1000.0), 240.0, 90.0, 200.0), Size::new(240.0, 150.0));
        assert_eq!(card_size(Size::new(4000.0, 100.0), 240.0, 90.0, 200.0).height, 90.0);
        assert_eq!(card_size(Size::new(100.0, 4000.0), 240.0, 90.0, 200.0).height, 200.0);
        assert_eq!(card_size(Size::ZERO, 240.0, 90.0, 200.0).height, 150.0);
    }

    #[test]
    fn pill_prefers_below_then_above_then_inside() {
        let screen = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let pill = Size::new(100.0, 24.0);
        let below = pill_frame(Rect::new(100.0, 100.0, 200.0, 200.0), pill, screen, 8.0);
        assert_eq!(below, Rect::new(150.0, 308.0, 100.0, 24.0));
        let above = pill_frame(Rect::new(100.0, 600.0, 200.0, 190.0), pill, screen, 8.0);
        assert_eq!(above.min_y(), 568.0);
        let inside = pill_frame(Rect::new(0.0, 0.0, 1000.0, 800.0), pill, screen, 8.0);
        assert_eq!(inside.min_y(), 768.0);
        let clamped = pill_frame(Rect::new(0.0, 100.0, 10.0, 10.0), pill, screen, 8.0);
        assert_eq!(clamped.min_x(), 0.0);
    }

    #[test]
    fn loupe_flips_near_edges() {
        let screen = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let s = Size::new(120.0, 140.0);
        assert_eq!(loupe_frame(Point::new(100.0, 100.0), s, screen, 20.0).origin, Point::new(120.0, 120.0));
        assert_eq!(loupe_frame(Point::new(950.0, 750.0), s, screen, 20.0).origin, Point::new(810.0, 590.0));
    }

    #[test]
    fn loupe_sampling_clamps() {
        assert_eq!(loupe_sample(50, 50, 9, 100, 100), Some(PixelRect { x: 46, y: 46, w: 9, h: 9 }));
        assert_eq!(loupe_sample(0, 99, 9, 100, 100), Some(PixelRect { x: 0, y: 91, w: 9, h: 9 }));
        assert_eq!(loupe_sample(2, 2, 9, 5, 5), Some(PixelRect { x: 0, y: 0, w: 5, h: 5 }));
        assert_eq!(loupe_sample(0, 0, 9, 0, 5), None);
    }

    #[test]
    fn fly_transform_fits() {
        let t = fly_transform(Rect::new(0.0, 0.0, 1000.0, 500.0), Rect::new(10.0, 10.0, 200.0, 200.0));
        assert_eq!(t.scale, 0.2);
        assert_eq!((t.dx, t.dy), (110.0 - 500.0, 110.0 - 250.0));
    }
}
