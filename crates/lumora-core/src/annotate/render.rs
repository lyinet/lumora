//! The display list: every annotation turned into a few primitive drawing
//! operations. The live editor maps ops to `CAShapeLayer`/`CATextLayer`
//! sublayers and the exporter replays them into a `CGContext`, so the two
//! can't drift apart — and the look of each tool is unit-testable here.

use crate::color::Rgb8;
use crate::geometry::{Point, Rect};

use super::document::Annotation;
use super::path::{
    arrow_centerline, arrow_outline, direction, head_points, point_along, polyline_lengths, smooth, sub_polyline,
    ArrowHead, Path,
};
use super::shape::{rect_radius, RedactMode, Shape, TEXT_PILL_PAD};
use super::style::{ArrowHeads, Dash, Fill, Style};

/// Opacity of highlighter ink. Higher than a normal-blend marker could
/// afford (0.38 before 0.8) because it multiplies: dark text under it stays
/// dark, only the paper takes the colour.
pub const HIGHLIGHTER_ALPHA: f64 = 0.55;
/// Dim applied outside a spotlight.
pub const SPOTLIGHT_DIM: f64 = 0.55;
/// Pixelation block / blur radius in document points.
pub const REDACT_BLOCK: f64 = 10.0;
pub const REDACT_BLUR: f64 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
}

/// How a path is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    Stroke {
        width: f64,
        cap: LineCap,
        /// `(on, off)` dash lengths; `None` = solid.
        dash: Option<(f64, f64)>,
    },
    /// `even_odd` punches holes (the spotlight).
    Fill { even_odd: bool },
}

/// How a shape composites onto what is under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    #[default]
    Normal,
    /// `result = under × ink`: colours light areas, leaves dark text dark
    /// (the highlighter). Live: `compositingFilter = multiplyBlendMode`;
    /// export: `kCGBlendModeMultiply`.
    Multiply,
}

/// A path drawn in one colour.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeOp {
    pub path: Path,
    pub paint: Paint,
    pub color: Rgb8,
    pub alpha: f64,
    /// Soft drop shadow (lifts ink off busy screenshots).
    pub shadow: bool,
    pub blend: Blend,
}

/// Text laid out in `rect` (top-left aligned, or centred for badges).
#[derive(Debug, Clone, PartialEq)]
pub struct TextOp {
    pub text: String,
    pub rect: Rect,
    pub font_size: f64,
    pub color: Rgb8,
    pub bold: bool,
    pub centered: bool,
    pub shadow: bool,
    /// 0–1, from the annotation's opacity.
    pub alpha: f64,
}

/// One drawing operation.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderOp {
    Shape(ShapeOp),
    Text(TextOp),
    /// Replace the base image inside `rect` with a pixelated / blurred copy.
    Redact {
        rect: Rect,
        mode: RedactMode,
        amount: f64,
    },
}

const WHITE: Rgb8 = Rgb8::new(255, 255, 255);
const BLACK: Rgb8 = Rgb8::new(0, 0, 0);

/// Text colour for badges and pills on `bg`. Deliberately biased toward
/// white (the macOS convention for saturated fills such as system blue,
/// which strict WCAG contrast would pair with black): only genuinely light
/// fills — yellow, white — get dark text.
pub fn contrast_on(bg: Rgb8) -> Rgb8 {
    if bg.luminance() > 0.5 {
        Rgb8::new(0x1C, 0x1C, 0x1E)
    } else {
        WHITE
    }
}

fn stroke(path: Path, style: &Style, width: f64, shadow: bool) -> RenderOp {
    stroke_dashed(path, style, width, None, shadow)
}

fn stroke_dashed(path: Path, style: &Style, width: f64, dash: Option<(f64, f64)>, shadow: bool) -> RenderOp {
    RenderOp::Shape(ShapeOp {
        path,
        paint: Paint::Stroke { width, cap: LineCap::Round, dash },
        color: style.color,
        alpha: 1.0,
        shadow,
        blend: Blend::Normal,
    })
}

fn fill(path: Path, color: Rgb8, alpha: f64, shadow: bool) -> RenderOp {
    RenderOp::Shape(ShapeOp {
        path,
        paint: Paint::Fill { even_odd: false },
        color,
        alpha,
        shadow,
        blend: Blend::Normal,
    })
}

/// An arrow. The classic look (filled head, solid) is one tapered filled
/// outline; every other combination is a constant-width shaft (dashed or
/// not) trimmed so it doesn't poke through the heads, plus the heads:
/// filled swallowtails or a stroked open chevron (never dashed).
fn arrow_ops(start: Point, end: Point, control: Option<Point>, style: &Style) -> Vec<RenderOp> {
    let w = style.width.points();
    if style.heads == ArrowHeads::Filled && style.dash == Dash::Solid {
        let outline = arrow_outline(start, end, control, w);
        if outline.is_empty() {
            return Vec::new();
        }
        return vec![fill(outline, style.color, 1.0, true)];
    }
    let line = arrow_centerline(start, end, control);
    let lengths = polyline_lengths(&line);
    let total = *lengths.last().unwrap_or(&0.0);
    if total <= 1e-6 {
        return Vec::new();
    }
    let both = style.heads == ArrowHeads::Both;
    let head = ArrowHead::for_width(w, if both { total / 2.0 } else { total });
    // Aim each head along the part of the curve it covers.
    let aim = |tip: Point, from: f64| direction(point_along(&line, &lengths, from), tip);
    let filled_trim = head.length * (1.0 - head.notch);
    let (trim_start, trim_end) = match style.heads {
        ArrowHeads::None => (0.0, 0.0),
        ArrowHeads::Filled => (0.0, filled_trim),
        ArrowHeads::Open => (0.0, w * 0.5),
        ArrowHeads::Both => (filled_trim, filled_trim),
    };
    let mut ops = Vec::new();
    let shaft = sub_polyline(&line, trim_start, total - trim_end);
    if shaft.len() >= 2 {
        ops.push(stroke_dashed(Path::polyline(&shaft), style, w, style.dash.pattern(w), true));
    }
    let filled_head = |tip: Point, dir: (f64, f64)| {
        let [l, t, r, n] = head_points(tip, dir, head);
        fill(Path::polygon(&[l, t, r, n]), style.color, 1.0, true)
    };
    match style.heads {
        ArrowHeads::None => {}
        ArrowHeads::Filled => ops.extend(aim(end, total - head.length).map(|d| filled_head(end, d))),
        ArrowHeads::Open => {
            if let Some(d) = aim(end, total - head.length) {
                let [l, t, r, _] = head_points(end, d, head);
                ops.push(stroke(Path::polyline(&[l, t, r]), style, w, true));
            }
        }
        ArrowHeads::Both => {
            ops.extend(aim(end, total - head.length).map(|d| filled_head(end, d)));
            ops.extend(aim(start, head.length).map(|d| filled_head(start, d)));
        }
    }
    ops
}

/// A closed outline, with the style's fill (if any) painted underneath
/// it. A solid fill carries the shadow so the stroke doesn't cast one onto
/// its own fill; a translucent fill casts none (it would muddy the inside).
fn filled_outline(path: Path, style: &Style, width: f64) -> Vec<RenderOp> {
    match style.fill.alpha() {
        None => vec![stroke_dashed(path, style, width, style.dash.pattern(width), true)],
        Some(alpha) => {
            let solid = style.fill == Fill::Solid;
            let outline = stroke_dashed(path.clone(), style, width, style.dash.pattern(width), !solid);
            vec![fill(path, style.color, alpha, solid), outline]
        }
    }
}

/// Ops for one annotation. `canvas` is the full image rect (the spotlight
/// dims all of it); `step` is the badge number for step annotations.
///
/// The annotation's opacity is folded into every op's `alpha` here, so the
/// live layers and the export (which both just honour `alpha`) can't
/// disagree. Redactions stay opaque whatever the opacity says.
pub fn render(a: &Annotation, canvas: Rect, step: Option<u32>) -> Vec<RenderOp> {
    let mut ops = render_opaque(a, canvas, step);
    apply_opacity(&mut ops, a.style.alpha());
    ops
}

/// Multiply every shape's and text's alpha by `alpha` (redactions are left
/// alone).
pub fn apply_opacity(ops: &mut [RenderOp], alpha: f64) {
    if alpha >= 1.0 {
        return;
    }
    for op in ops {
        match op {
            RenderOp::Shape(s) => s.alpha *= alpha,
            RenderOp::Text(t) => t.alpha *= alpha,
            RenderOp::Redact { .. } => {}
        }
    }
}

fn render_opaque(a: &Annotation, canvas: Rect, step: Option<u32>) -> Vec<RenderOp> {
    let style = &a.style;
    let w = style.width.points();
    match &a.shape {
        Shape::Arrow { start, end, control } => arrow_ops(*start, *end, *control, style),
        Shape::Line { start, end } => {
            vec![stroke_dashed(Path::polyline(&[*start, *end]), style, w, style.dash.pattern(w), true)]
        }
        Shape::Rect { rect, rounded } => {
            let path = if *rounded { Path::rounded_rect(*rect, rect_radius(style)) } else { Path::rect(*rect) };
            filled_outline(path, style, w)
        }
        Shape::Ellipse { rect } => filled_outline(Path::ellipse(*rect), style, w),
        Shape::Pen { points } => vec![stroke(smooth(points), style, w, true)],
        Shape::Highlighter { points } => vec![RenderOp::Shape(ShapeOp {
            path: smooth(points),
            paint: Paint::Stroke { width: style.width.highlighter_points(), cap: LineCap::Butt, dash: None },
            color: style.color,
            alpha: HIGHLIGHTER_ALPHA,
            shadow: false,
            blend: Blend::Multiply,
        })],
        Shape::Text { text, background, .. } => {
            let Some(rect) = a.shape.text_rect(style) else { return Vec::new() };
            let f = style.width.font_size();
            let mut ops = Vec::new();
            let color = if *background {
                let pill = rect.inset(-f * TEXT_PILL_PAD.0, -f * TEXT_PILL_PAD.1);
                ops.push(fill(Path::rounded_rect(pill, f * 0.45), style.color, 1.0, true));
                contrast_on(style.color)
            } else {
                style.color
            };
            ops.push(RenderOp::Text(TextOp {
                text: text.clone(),
                rect,
                font_size: f,
                color,
                bold: true,
                centered: false,
                shadow: !*background,
                alpha: 1.0,
            }));
            ops
        }
        Shape::Step { center } => {
            let d = style.width.badge_diameter();
            let r = d / 2.0;
            let n = step.unwrap_or(0);
            vec![
                fill(Path::circle(*center, r), style.color, 1.0, true),
                RenderOp::Shape(ShapeOp {
                    path: Path::circle(*center, r - 1.0),
                    paint: Paint::Stroke { width: 2.0, cap: LineCap::Round, dash: None },
                    color: WHITE,
                    alpha: 0.9,
                    shadow: false,
                    blend: Blend::Normal,
                }),
                RenderOp::Text(TextOp {
                    text: n.to_string(),
                    rect: Rect::new(center.x - r, center.y - r, d, d),
                    font_size: if n >= 10 { d * 0.46 } else { d * 0.56 },
                    color: contrast_on(style.color),
                    bold: true,
                    centered: true,
                    shadow: false,
                    alpha: 1.0,
                }),
            ]
        }
        Shape::Redact { rect, mode } => {
            let amount = match mode {
                RedactMode::Pixelate => REDACT_BLOCK,
                RedactMode::Blur => REDACT_BLUR,
            };
            vec![RenderOp::Redact { rect: rect.normalized(), mode: *mode, amount }]
        }
        Shape::Spotlight { rect, ellipse } => {
            let mut path = Path::rect(canvas.inset(-1.0, -1.0));
            let hole = if *ellipse { Path::ellipse(*rect) } else { Path::rounded_rect(*rect, 6.0) };
            path.extend(&hole);
            vec![RenderOp::Shape(ShapeOp {
                path,
                paint: Paint::Fill { even_odd: true },
                color: BLACK,
                alpha: SPOTLIGHT_DIM,
                shadow: false,
                blend: Blend::Normal,
            })]
        }
    }
}

/// Bounds of everything an op list paints (for layer frames), padded for
/// stroke width and shadow.
pub fn ops_bounds(ops: &[RenderOp]) -> Option<Rect> {
    let mut acc: Option<Rect> = None;
    for op in ops {
        let r = match op {
            RenderOp::Shape(s) => {
                let pad = match s.paint {
                    Paint::Stroke { width, .. } => width / 2.0 + 1.0,
                    Paint::Fill { .. } => 1.0,
                } + if s.shadow { 4.0 } else { 0.0 };
                s.path.control_bounds().map(|b| b.inset(-pad, -pad))
            }
            RenderOp::Text(t) => Some(t.rect.inset(-4.0, -4.0)),
            RenderOp::Redact { rect, .. } => Some(*rect),
        };
        if let Some(r) = r {
            acc = Some(acc.map_or(r, |a| a.union(&r)));
        }
    }
    acc
}

/// A point that is visibly inside an op list (used by tests and to place
/// accessibility focus).
pub fn anchor_point(ops: &[RenderOp]) -> Option<Point> {
    ops_bounds(ops).map(|b| b.mid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::style::{StrokeWidth, FILL_TRANSLUCENT_ALPHA, PALETTE};
    use crate::geometry::Size;

    fn ann(shape: Shape) -> Annotation {
        Annotation { id: 1, shape, style: Style::new(PALETTE[3], StrokeWidth::Medium) }
    }

    const CANVAS: Rect = Rect::new(0.0, 0.0, 400.0, 300.0);

    #[test]
    fn arrows_are_one_filled_outline() {
        let ops = render(
            &ann(Shape::Arrow { start: Point::new(0.0, 0.0), end: Point::new(100.0, 0.0), control: None }),
            CANVAS,
            None,
        );
        assert_eq!(ops.len(), 1);
        assert!(
            matches!(&ops[0], RenderOp::Shape(s) if matches!(s.paint, Paint::Fill { even_odd: false }) && s.shadow)
        );
        let zero = render(
            &ann(Shape::Arrow { start: Point::new(5.0, 5.0), end: Point::new(5.0, 5.0), control: None }),
            CANVAS,
            None,
        );
        assert!(zero.is_empty());
    }

    #[test]
    fn highlighter_is_translucent_and_wide() {
        let ops = render(&ann(Shape::Highlighter { points: vec![Point::ZERO, Point::new(50.0, 0.0)] }), CANVAS, None);
        match &ops[0] {
            RenderOp::Shape(s) => {
                assert_eq!(s.alpha, HIGHLIGHTER_ALPHA);
                assert_eq!(s.blend, Blend::Multiply);
                assert!(!s.shadow, "a shadow would darken the text it marks");
                assert_eq!(s.paint, Paint::Stroke { width: 20.0, cap: LineCap::Butt, dash: None });
            }
            _ => panic!(),
        }
    }

    #[test]
    fn only_the_highlighter_multiplies() {
        let all = [
            Shape::Arrow { start: Point::ZERO, end: Point::new(50.0, 0.0), control: None },
            Shape::Line { start: Point::ZERO, end: Point::new(50.0, 0.0) },
            Shape::Rect { rect: Rect::new(0.0, 0.0, 9.0, 9.0), rounded: false },
            Shape::Pen { points: vec![Point::ZERO, Point::new(9.0, 9.0)] },
            Shape::Step { center: Point::new(20.0, 20.0) },
            Shape::Spotlight { rect: Rect::new(0.0, 0.0, 9.0, 9.0), ellipse: true },
        ];
        for shape in all {
            for op in render(&ann(shape), CANVAS, Some(1)) {
                if let RenderOp::Shape(s) = op {
                    assert_eq!(s.blend, Blend::Normal);
                }
            }
        }
    }

    /// Multiply on 8-bit channels, as the compositors do it.
    fn multiply(under: Rgb8, ink: Rgb8, alpha: f64) -> Rgb8 {
        let ch = |u: u8, i: u8| {
            let (u, i) = (f64::from(u) / 255.0, f64::from(i) / 255.0);
            ((u * (1.0 - alpha) + u * i * alpha) * 255.0).round() as u8
        };
        Rgb8::new(ch(under.r, ink.r), ch(under.g, ink.g), ch(under.b, ink.b))
    }

    #[test]
    fn multiplied_highlight_keeps_text_dark_and_paper_coloured() {
        let yellow = PALETTE[1];
        let text = Rgb8::new(0x1C, 0x1C, 0x1E);
        let paper = Rgb8::new(255, 255, 255);
        let over_text = multiply(text, yellow, HIGHLIGHTER_ALPHA);
        let over_paper = multiply(paper, yellow, HIGHLIGHTER_ALPHA);
        // Text is never lightened (normal blending at this alpha would lift
        // it towards yellow) and keeps strong contrast with the highlight.
        assert!(over_text.luminance() <= text.luminance() + 1e-9);
        let contrast = (over_paper.luminance() + 0.05) / (over_text.luminance() + 0.05);
        assert!(contrast > 7.0, "WCAG AAA: {contrast} ({over_paper:?} vs {over_text:?})");
        assert!(over_paper.b < 200, "paper takes the colour: {over_paper:?}");
    }

    #[test]
    fn steps_show_their_number_in_a_contrasting_colour() {
        let mut a = ann(Shape::Step { center: Point::new(50.0, 50.0) });
        let ops = render(&a, CANVAS, Some(3));
        assert_eq!(ops.len(), 3);
        match &ops[2] {
            RenderOp::Text(t) => {
                assert_eq!(t.text, "3");
                assert!(t.centered);
                assert_eq!(t.color, WHITE, "white on blue");
                assert_eq!(t.rect.mid(), Point::new(50.0, 50.0));
            }
            _ => panic!(),
        }
        a.style.color = PALETTE[1]; // yellow
        match &render(&a, CANVAS, Some(12))[2] {
            RenderOp::Text(t) => {
                assert_ne!(t.color, WHITE, "dark on yellow");
                assert!(t.font_size < 32.0 * 0.5, "two digits shrink");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn text_with_pill_draws_background_first() {
        let shape = Shape::Text {
            origin: Point::new(10.0, 10.0),
            text: "Note".into(),
            size: Size::new(40.0, 20.0),
            background: true,
        };
        let ops = render(&ann(shape.clone()), CANVAS, None);
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], RenderOp::Shape(_)));
        assert!(matches!(&ops[1], RenderOp::Text(t) if t.color == WHITE && !t.shadow));
        let Shape::Text { origin, text, size, .. } = shape else { unreachable!() };
        let plain = Shape::Text { origin, text, size, background: false };
        let ops = render(&ann(plain), CANVAS, None);
        assert!(matches!(&ops[..], [RenderOp::Text(t)] if t.color == PALETTE[3] && t.shadow));
    }

    #[test]
    fn spotlight_dims_the_canvas_with_a_hole() {
        let ops =
            render(&ann(Shape::Spotlight { rect: Rect::new(100.0, 100.0, 50.0, 50.0), ellipse: false }), CANVAS, None);
        match &ops[0] {
            RenderOp::Shape(s) => {
                assert_eq!(s.paint, Paint::Fill { even_odd: true });
                assert_eq!(s.path.flatten(4).len(), 2, "outer rect + hole");
                assert!(s.path.control_bounds().unwrap().contains_rect(&CANVAS));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn redaction_carries_mode_and_amount() {
        let ops =
            render(&ann(Shape::Redact { rect: Rect::new(0.0, 0.0, 10.0, 10.0), mode: RedactMode::Blur }), CANVAS, None);
        assert_eq!(
            ops,
            vec![RenderOp::Redact {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                mode: RedactMode::Blur,
                amount: REDACT_BLUR
            }]
        );
    }

    fn arrow(heads: ArrowHeads, dash: Dash) -> Vec<RenderOp> {
        let mut a = ann(Shape::Arrow { start: Point::new(0.0, 0.0), end: Point::new(100.0, 0.0), control: None });
        a.style.heads = heads;
        a.style.dash = dash;
        render(&a, CANVAS, None)
    }

    fn shapes(ops: &[RenderOp]) -> Vec<&ShapeOp> {
        ops.iter().filter_map(|o| if let RenderOp::Shape(s) = o { Some(s) } else { None }).collect()
    }

    #[test]
    fn arrowhead_styles() {
        // Classic: one tapered filled outline (unchanged from 0.7).
        assert_eq!(arrow(ArrowHeads::Filled, Dash::Solid).len(), 1);
        // No head: just a round-capped shaft over the full length.
        let none = arrow(ArrowHeads::None, Dash::Solid);
        let s = shapes(&none);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].paint, Paint::Stroke { width: 4.0, cap: LineCap::Round, dash: None });
        assert_eq!(s[0].path.control_bounds().unwrap().max_x(), 100.0);
        // Open: shaft + a stroked chevron whose tip is the arrow's end.
        let open = arrow(ArrowHeads::Open, Dash::Solid);
        let s = shapes(&open);
        assert_eq!(s.len(), 2);
        assert!(matches!(s[1].paint, Paint::Stroke { dash: None, .. }));
        assert_eq!(s[1].path.flatten(1)[0][1], Point::new(100.0, 0.0));
        // Both: shaft trimmed at both ends + two filled heads.
        let both = arrow(ArrowHeads::Both, Dash::Solid);
        let s = shapes(&both);
        assert_eq!(s.len(), 3);
        let shaft = s[0].path.control_bounds().unwrap();
        assert!(shaft.min_x() > 1.0 && shaft.max_x() < 99.0, "{shaft:?}");
        let tips: Vec<Point> = s[1..].iter().map(|h| h.path.flatten(1)[0][1]).collect();
        assert_eq!(tips, vec![Point::new(100.0, 0.0), Point::new(0.0, 0.0)]);
        assert!(s[1..].iter().all(|h| h.paint == Paint::Fill { even_odd: false }));
        // A dashed classic arrow switches to shaft + filled head; the head is never dashed.
        let dashed = arrow(ArrowHeads::Filled, Dash::Dashed);
        let s = shapes(&dashed);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].paint, Paint::Stroke { width: 4.0, cap: LineCap::Round, dash: Some((12.0, 8.0)) });
        assert_eq!(s[1].paint, Paint::Fill { even_odd: false });
        // Zero length draws nothing in any style.
        for h in ArrowHeads::ALL {
            let mut a = ann(Shape::Arrow { start: Point::new(5.0, 5.0), end: Point::new(5.0, 5.0), control: None });
            a.style.heads = h;
            assert!(render(&a, CANVAS, None).is_empty(), "{h:?}");
        }
    }

    #[test]
    fn curved_heads_follow_the_curve() {
        let mut a = ann(Shape::Arrow {
            start: Point::new(0.0, 0.0),
            end: Point::new(100.0, 0.0),
            control: Some(Point::new(50.0, 80.0)),
        });
        a.style.heads = ArrowHeads::Both;
        let ops = render(&a, CANVAS, None);
        let s = shapes(&ops);
        // The end head points down-right-ish along the curve's tail (coming up
        // from y > 0), so its wings sit below the tip.
        let end_head = &s[1].path.flatten(1)[0];
        assert!(end_head[0].y > 0.0 && end_head[2].y > 0.0, "{end_head:?}");
    }

    #[test]
    fn dashes_apply_to_lines_and_outlines_but_not_freehand() {
        for shape in [
            Shape::Line { start: Point::ZERO, end: Point::new(50.0, 0.0) },
            Shape::Rect { rect: Rect::new(0.0, 0.0, 50.0, 50.0), rounded: false },
            Shape::Ellipse { rect: Rect::new(0.0, 0.0, 50.0, 50.0) },
        ] {
            let mut a = ann(shape);
            a.style.dash = Dash::Dotted;
            a.style.fill = Fill::Translucent;
            let ops = render(&a, CANVAS, None);
            let stroke = shapes(&ops).into_iter().find(|s| matches!(s.paint, Paint::Stroke { .. })).unwrap();
            assert_eq!(stroke.paint, Paint::Stroke { width: 4.0, cap: LineCap::Round, dash: Some((0.0, 8.0)) });
        }
        let mut pen = ann(Shape::Pen { points: vec![Point::ZERO, Point::new(9.0, 9.0)] });
        pen.style.dash = Dash::Dashed;
        assert!(
            matches!(&render(&pen, CANVAS, None)[0], RenderOp::Shape(s) if matches!(s.paint, Paint::Stroke { dash: None, .. }))
        );
    }

    #[test]
    fn rect_and_ellipse_fills_sit_under_the_outline() {
        let rect = Shape::Rect { rect: Rect::new(10.0, 10.0, 50.0, 30.0), rounded: true };
        let mut a = ann(rect);
        assert_eq!(render(&a, CANVAS, None).len(), 1, "no fill by default");
        for (fill_mode, alpha, shadow) in [(Fill::Solid, 1.0, true), (Fill::Translucent, FILL_TRANSLUCENT_ALPHA, false)]
        {
            a.style.fill = fill_mode;
            let ops = render(&a, CANVAS, None);
            match &ops[..] {
                [RenderOp::Shape(f), RenderOp::Shape(s)] => {
                    assert_eq!(f.paint, Paint::Fill { even_odd: false });
                    assert_eq!(f.alpha, alpha);
                    assert_eq!(f.shadow, shadow);
                    assert_eq!(f.path, s.path, "fill and outline share the path");
                    assert!(matches!(s.paint, Paint::Stroke { .. }));
                    assert_eq!(s.shadow, !shadow);
                    assert_eq!(f.color, s.color);
                }
                other => panic!("{other:?}"),
            }
            // Opacity compounds with the fill's own alpha.
            a.style.opacity = 50;
            assert!(
                matches!(&render(&a, CANVAS, None)[0], RenderOp::Shape(f) if (f.alpha - alpha * 0.5).abs() < 1e-12)
            );
            a.style.opacity = 100;
        }
        let mut e = ann(Shape::Ellipse { rect: Rect::new(0.0, 0.0, 40.0, 20.0) });
        e.style.fill = Fill::Translucent;
        assert_eq!(render(&e, CANVAS, None).len(), 2);
        // Kinds without a fill ignore it.
        let mut l = ann(Shape::Line { start: Point::ZERO, end: Point::new(9.0, 9.0) });
        l.style.fill = Fill::Solid;
        assert_eq!(render(&l, CANVAS, None).len(), 1);
    }

    #[test]
    fn opacity_scales_every_op_but_redactions() {
        let mut a = ann(Shape::Step { center: Point::new(50.0, 50.0) });
        let opaque = render(&a, CANVAS, Some(1));
        a.style.opacity = 50;
        let half = render(&a, CANVAS, Some(1));
        assert_eq!(opaque.len(), half.len());
        for (o, h) in opaque.iter().zip(&half) {
            match (o, h) {
                (RenderOp::Shape(o), RenderOp::Shape(h)) => {
                    assert!((h.alpha - o.alpha * 0.5).abs() < 1e-12);
                    assert_eq!(o.path, h.path, "geometry unchanged");
                }
                (RenderOp::Text(o), RenderOp::Text(h)) => assert!((h.alpha - o.alpha * 0.5).abs() < 1e-12),
                _ => panic!("op kinds differ"),
            }
        }
        // The highlighter's own translucency compounds with the opacity.
        let mut hl = ann(Shape::Highlighter { points: vec![Point::ZERO, Point::new(9.0, 0.0)] });
        hl.style.opacity = 50;
        assert!(
            matches!(&render(&hl, CANVAS, None)[0], RenderOp::Shape(s) if (s.alpha - HIGHLIGHTER_ALPHA * 0.5).abs() < 1e-12)
        );
        // Redactions never become see-through, and a bogus 0 % is clamped.
        let mut r = ann(Shape::Redact { rect: Rect::new(0.0, 0.0, 10.0, 10.0), mode: RedactMode::Pixelate });
        r.style.opacity = 10;
        assert_eq!(render(&r, CANVAS, None), render(&ann(r.shape.clone()), CANVAS, None));
        let mut zero = ann(Shape::Line { start: Point::ZERO, end: Point::new(10.0, 0.0) });
        zero.style.opacity = 0;
        assert!(matches!(&render(&zero, CANVAS, None)[0], RenderOp::Shape(s) if (s.alpha - 0.1).abs() < 1e-12));
    }

    #[test]
    fn bounds_include_stroke_and_shadow() {
        let ops =
            render(&ann(Shape::Line { start: Point::new(10.0, 10.0), end: Point::new(20.0, 10.0) }), CANVAS, None);
        let b = ops_bounds(&ops).unwrap();
        assert!(b.contains_rect(&Rect::new(10.0, 8.0, 10.0, 4.0)));
        assert_eq!(anchor_point(&ops), Some(Point::new(15.0, 10.0)));
        assert_eq!(ops_bounds(&[]), None);
    }
}
