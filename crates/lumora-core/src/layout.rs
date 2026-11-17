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

/// How many leading toolbar items stay visible in `available` points.
///
/// `advances[i]` is item `i`'s width plus the space after it, in toolbar
/// order. When every item fits, all stay. Otherwise the trailing items move
/// into an overflow ("…") menu, whose button needs `more_width`; the
/// result is the longest prefix that fits next to that button (possibly 0).
pub fn toolbar_overflow(advances: &[f64], available: f64, more_width: f64) -> usize {
    let total: f64 = advances.iter().sum();
    if total <= available + 0.5 {
        return advances.len();
    }
    let room = available - more_width;
    let mut used = 0.0;
    for (i, a) in advances.iter().enumerate() {
        used += a;
        if used > room + 0.5 {
            return i;
        }
    }
    advances.len()
}

/// Placement of a centred row of round buttons (Quick Access hover row).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButtonRow {
    /// Button diameter.
    pub size: f64,
    pub gap: f64,
    /// Left edge of the first button.
    pub x: f64,
    /// Bottom edge of the row (AppKit, y up).
    pub y: f64,
}

/// Lay out `count` round buttons centred in `area` (AppKit, y up). Buttons
/// shrink from `preferred` down to `min`, and gaps from `gap` down to half,
/// so the row keeps `margin` on both sides. The bottom `reserved` points (a
/// duration badge) are kept clear when the height allows; otherwise the row
/// is simply centred.
pub fn button_row(
    count: usize,
    area: Size,
    preferred: f64,
    min: f64,
    gap: f64,
    margin: f64,
    reserved: f64,
) -> ButtonRow {
    let n = count.max(1) as f64;
    let avail = (area.width - 2.0 * margin).max(0.0);
    let width = |size: f64, gap: f64| n * size + (n - 1.0) * gap;
    let (mut size, mut g) = (preferred, gap);
    if width(size, g) > avail {
        g = (gap / 2.0).max(0.0);
        size = ((avail - (n - 1.0) * g) / n).clamp(min.min(preferred), preferred);
    }
    let x = (area.width - width(size, g)) / 2.0;
    let centred = (area.height - size) / 2.0;
    // Centre in the space above the reserved strip when the row fits there
    // with a few points to spare; never push it off the top.
    let above = reserved + (area.height - reserved - size) / 2.0;
    let y = if reserved > 0.0 && centred < reserved + 4.0 && above + size <= area.height - 4.0 {
        above.max(centred)
    } else {
        centred
    };
    ButtonRow { size, gap: g, x, y }
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

    #[test]
    fn toolbar_overflow_keeps_longest_prefix() {
        let items = [30.0, 30.0, 30.0, 100.0, 50.0];
        assert_eq!(toolbar_overflow(&items, 240.0, 32.0), 5);
        assert_eq!(toolbar_overflow(&items, 1000.0, 32.0), 5);
        // 239 < 240 total: overflow; room = 207 → 30+30+30+100 = 190 fits.
        assert_eq!(toolbar_overflow(&items, 239.0, 32.0), 4);
        assert_eq!(toolbar_overflow(&items, 150.0, 32.0), 3);
        assert_eq!(toolbar_overflow(&items, 40.0, 32.0), 0);
        assert_eq!(toolbar_overflow(&items, -5.0, 32.0), 0);
        assert_eq!(toolbar_overflow(&[], 10.0, 32.0), 0);
        // Monotonic in the available width.
        let mut last = 0;
        for w in 0..400 {
            let n = toolbar_overflow(&items, f64::from(w), 32.0);
            assert!(n >= last);
            last = n;
        }
    }
    #[test]
    fn button_row_fits_and_clears_the_badge() {
        // Five 30 pt buttons fit a 220 pt thumbnail unchanged, centred.
        let r = button_row(5, Size::new(220.0, 118.0), 30.0, 24.0, 6.0, 8.0, 32.0);
        assert_eq!((r.size, r.gap), (30.0, 6.0));
        assert!((r.x - (220.0 - 174.0) / 2.0).abs() < 1e-9);
        assert!((r.y - 44.0).abs() < 1e-9, "tall enough: plain centring clears the badge");
        // Short (wide-aspect) card: the row moves above the badge.
        let r = button_row(5, Size::new(220.0, 84.0), 30.0, 24.0, 6.0, 8.0, 32.0);
        assert!(r.y >= 32.0 && r.y + r.size <= 84.0 - 4.0, "{r:?}");
        // Too narrow: gaps halve, buttons shrink, margins hold.
        let r = button_row(6, Size::new(180.0, 84.0), 30.0, 24.0, 6.0, 8.0, 0.0);
        assert!(r.size < 30.0 && r.size >= 24.0 && r.gap == 3.0, "{r:?}");
        assert!(r.x >= 8.0 - 1e-9);
        // No reserved strip: centred.
        let r = button_row(4, Size::new(220.0, 84.0), 30.0, 24.0, 6.0, 8.0, 0.0);
        assert!((r.y - 27.0).abs() < 1e-9);
    }
}
