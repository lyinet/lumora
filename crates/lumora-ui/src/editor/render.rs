//! Drawing the annotation display list ([`lumora_core::annotate::render`])
//! two ways from the same ops:
//!
//! * **Live** — one container `CALayer` per annotation holding
//!   `CAShapeLayer`s (paths), `CATextLayer`s (text) and plain layers whose
//!   contents are Core Image renders (redactions). Rebuilt only when that
//!   annotation changes.
//! * **Export** — replayed into a `CGBitmapContext` at the capture's
//!   native scale, cropped, for ⌘C / ⌘S / drag-out.
//!
//! Both work in document points with y down; the export context is flipped
//! and scaled once so the ops need no conversion.

use std::cell::OnceCell;

use lumora_core::annotate::document::Document;
use lumora_core::annotate::render::{LineCap, Paint, RenderOp, ShapeOp, TextOp};
use lumora_core::annotate::shape::RedactMode;
use lumora_core::geometry::Rect;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSGraphicsContext, NSStringDrawing};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality, CGLineCap, CGLineJoin,
};
use objc2_core_image::{
    kCIInputCenterKey, kCIInputImageKey, kCIInputRadiusKey, kCIInputScaleKey, CIContext, CIFilter, CIImage, CIVector,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSObjectNSKeyValueCoding, NSString};
use objc2_quartz_core::{
    kCAAlignmentCenter, kCAAlignmentLeft, kCAFillRuleEvenOdd, kCAFillRuleNonZero, kCALineCapButt, kCALineCapRound,
    kCALineJoinRound, CALayer, CAShapeLayer, CATextLayer,
};

use crate::util::{cf_as_object, cg_path, cg_rect, cg_rgb8, weight};

/// Shadow under ink: black @ 30 %, 3 pt blur, 1 pt down.
const SHADOW_OPACITY: f32 = 0.3;
const SHADOW_RADIUS: f64 = 1.5;
const SHADOW_BLUR: f64 = 3.0;
const SHADOW_DY: f64 = 1.0;

/// The bold system font used for annotation text and badge numbers.
pub fn annotation_font(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight::BOLD)
}

// ---------------------------------------------------------------------------
// Redaction via Core Image
// ---------------------------------------------------------------------------

thread_local! {
    static CI: OnceCell<Retained<CIContext>> = const { OnceCell::new() };
}

fn ci_context() -> Retained<CIContext> {
    // SAFETY: `+[CIContext context]` has no preconditions.
    CI.with(|c| c.get_or_init(|| unsafe { CIContext::context() }).clone())
}

/// The base image's pixels inside `rect` (document points), pixelated or
/// blurred. `amount` is the block size / blur radius in points. Returns an
/// image exactly covering `rect` (clipped to the image).
pub fn redacted(base: &CGImage, scale: f64, rect: Rect, mode: RedactMode, amount: f64) -> Option<CFRetained<CGImage>> {
    let (pw, ph) = (CGImage::width(Some(base)) as f64, CGImage::height(Some(base)) as f64);
    let px = Rect::new(rect.min_x() * scale, rect.min_y() * scale, rect.width() * scale, rect.height() * scale)
        .integral(1.0)
        .intersection(&Rect::new(0.0, 0.0, pw, ph))?;
    // Core Image is y-up.
    let ci_rect = Rect::new(px.min_x(), ph - px.max_y(), px.width(), px.height());
    // SAFETY: standard Core Image filter setup; keys and value types are the
    // documented ones for CIPixellate / CIGaussianBlur.
    unsafe {
        let input = CIImage::imageWithCGImage(base).imageByClampingToExtent();
        let (name, key, value): (&str, &NSString, Retained<NSNumber>) = match mode {
            RedactMode::Pixelate => {
                ("CIPixellate", kCIInputScaleKey, NSNumber::numberWithDouble((amount * scale).max(2.0)))
            }
            RedactMode::Blur => ("CIGaussianBlur", kCIInputRadiusKey, NSNumber::numberWithDouble(amount * scale)),
        };
        let filter = CIFilter::filterWithName(&NSString::from_str(name))?;
        filter.setValue_forKey(Some(&input), kCIInputImageKey);
        filter.setValue_forKey(Some(&value), key);
        if mode == RedactMode::Pixelate {
            // Anchor the block grid at the region's corner so blocks don't
            // straddle its edge.
            let center = CIVector::vectorWithX_Y(ci_rect.min_x(), ci_rect.min_y());
            filter.setValue_forKey(Some(&center), kCIInputCenterKey);
        }
        let output = filter.outputImage()?;
        let image = ci_context().createCGImage_fromRect(&output, cg_rect(ci_rect))?;
        Some(CFRetained::from(image))
    }
}

/// Memoises redaction renders by (rect, mode) so dragging other
/// annotations doesn't re-run Core Image.
#[derive(Default)]
pub struct RedactionCache {
    entries: Vec<(Rect, RedactMode, CFRetained<CGImage>)>,
}

impl std::fmt::Debug for RedactionCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RedactionCache({})", self.entries.len())
    }
}

impl RedactionCache {
    const MAX: usize = 32;

    pub fn get(
        &mut self,
        base: &CGImage,
        scale: f64,
        rect: Rect,
        mode: RedactMode,
        amount: f64,
    ) -> Option<CFRetained<CGImage>> {
        if let Some((_, _, img)) = self.entries.iter().find(|(r, m, _)| *r == rect && *m == mode) {
            return Some(img.clone());
        }
        let img = redacted(base, scale, rect, mode, amount)?;
        if self.entries.len() >= Self::MAX {
            self.entries.remove(0);
        }
        self.entries.push((rect, mode, img.clone()));
        Some(img)
    }
}

// ---------------------------------------------------------------------------
// Live layers
// ---------------------------------------------------------------------------

/// What live layers need besides the ops.
pub struct LayerContext<'a> {
    pub base: &'a CGImage,
    pub scale: f64,
    /// Full canvas in points (container frames).
    pub canvas: Rect,
    /// `contentsScale` for text layers (backing scale × zoom, ≥ 1).
    pub text_scale: f64,
    pub redactions: &'a mut RedactionCache,
}

fn apply_shadow(layer: &CALayer) {
    layer.setShadowColor(Some(&cg_rgb8(lumora_core::color::Rgb8::new(0, 0, 0), 1.0)));
    layer.setShadowOpacity(SHADOW_OPACITY);
    layer.setShadowRadius(SHADOW_RADIUS);
    // The container is geometry-flipped (y down), so +y is "down".
    layer.setShadowOffset(CGSize::new(0.0, SHADOW_DY));
}

fn shape_layer(op: &ShapeOp, canvas: Rect) -> Retained<CAShapeLayer> {
    let l = CAShapeLayer::new();
    l.setFrame(cg_rect(canvas));
    l.setPath(Some(&cg_path(&op.path)));
    match op.paint {
        Paint::Stroke { width, cap, dash } => {
            l.setFillColor(None);
            l.setStrokeColor(Some(&cg_rgb8(op.color, 1.0)));
            l.setLineWidth(width);
            if let Some((on, off)) = dash {
                let pattern = [NSNumber::numberWithDouble(on), NSNumber::numberWithDouble(off)];
                l.setLineDashPattern(Some(&NSArray::from_retained_slice(&pattern)));
            }
            // SAFETY: valid static CA constants.
            unsafe {
                l.setLineCap(match cap {
                    LineCap::Round => kCALineCapRound,
                    LineCap::Butt => kCALineCapButt,
                });
                l.setLineJoin(kCALineJoinRound);
            }
        }
        Paint::Fill { even_odd } => {
            l.setStrokeColor(None);
            l.setFillColor(Some(&cg_rgb8(op.color, 1.0)));
            // SAFETY: valid static CA constants.
            unsafe { l.setFillRule(if even_odd { kCAFillRuleEvenOdd } else { kCAFillRuleNonZero }) };
        }
    }
    l.setOpacity(op.alpha as f32);
    if op.shadow {
        apply_shadow(&l);
    }
    l
}

fn text_layer(op: &TextOp, scale: f64) -> Retained<CATextLayer> {
    let font = annotation_font(op.font_size);
    let t = CATextLayer::new();
    // SAFETY: NSFont is toll-free bridged to CTFont.
    unsafe { t.setFont(Some(&*(&*font as *const NSFont as *const objc2_core_foundation::CFType))) };
    t.setFontSize(op.font_size);
    t.setForegroundColor(Some(&cg_rgb8(op.color, 1.0)));
    t.setContentsScale(scale);
    t.setWrapped(true);
    let mut frame = op.rect;
    if op.centered {
        // CATextLayer has no vertical centring: offset by the line height.
        let line = crate::util::text_size("8", &font).1;
        frame = Rect::new(op.rect.min_x(), op.rect.mid().y - line / 2.0, op.rect.width(), line);
        // SAFETY: valid static CA constant.
        unsafe { t.setAlignmentMode(kCAAlignmentCenter) };
    } else {
        // Leave room so the last glyph never wraps due to rounding.
        frame = Rect::new(frame.min_x(), frame.min_y(), frame.width() + 2.0, frame.height());
        // SAFETY: valid static CA constant.
        unsafe { t.setAlignmentMode(kCAAlignmentLeft) };
    }
    t.setFrame(cg_rect(frame));
    // SAFETY: NSString is a valid `string`.
    unsafe { t.setString(Some(&NSString::from_str(&op.text))) };
    if op.shadow {
        apply_shadow(&t);
    }
    t
}

/// A container layer (frame = canvas) drawing `ops`.
pub fn build_layer(ops: &[RenderOp], ctx: &mut LayerContext<'_>) -> Retained<CALayer> {
    let container = CALayer::new();
    container.setFrame(cg_rect(ctx.canvas));
    for op in ops {
        match op {
            RenderOp::Shape(s) => container.addSublayer(&shape_layer(s, ctx.canvas)),
            RenderOp::Text(t) => container.addSublayer(&text_layer(t, ctx.text_scale)),
            RenderOp::Redact { rect, mode, amount } => {
                let l = CALayer::new();
                // The render covers only the part of the rect on the image.
                l.setFrame(cg_rect(rect.intersection(&ctx.canvas).unwrap_or(*rect)));
                if let Some(img) = ctx.redactions.get(ctx.base, ctx.scale, *rect, *mode, *amount) {
                    // SAFETY: a CGImage is valid layer contents.
                    unsafe { l.setContents(Some(cf_as_object(&*img))) };
                    // The render is in image pixels, already flipped by CG.
                    l.setContentsGravity(unsafe { objc2_quartz_core::kCAGravityResize });
                }
                container.addSublayer(&l);
            }
        }
    }
    container
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

fn draw_shape(ctx: &CGContext, op: &ShapeOp, scale: f64) {
    CGContext::save_g_state(Some(ctx));
    CGContext::set_alpha(Some(ctx), op.alpha);
    if op.shadow {
        // Shadow geometry is in device space (pixels, y up), unaffected by
        // the flipped, scaled CTM.
        CGContext::set_shadow_with_color(
            Some(ctx),
            CGSize::new(0.0, -SHADOW_DY * scale),
            SHADOW_BLUR * scale,
            Some(&cg_rgb8(lumora_core::color::Rgb8::new(0, 0, 0), f64::from(SHADOW_OPACITY))),
        );
    }
    CGContext::add_path(Some(ctx), Some(&cg_path(&op.path)));
    match op.paint {
        Paint::Stroke { width, cap, dash } => {
            CGContext::set_stroke_color_with_color(Some(ctx), Some(&cg_rgb8(op.color, 1.0)));
            CGContext::set_line_width(Some(ctx), width);
            if let Some((on, off)) = dash {
                let lengths = [on, off];
                // SAFETY: `lengths` outlives the call; count matches.
                unsafe { CGContext::set_line_dash(Some(ctx), 0.0, lengths.as_ptr(), lengths.len()) };
            }
            CGContext::set_line_cap(Some(ctx), if cap == LineCap::Round { CGLineCap::Round } else { CGLineCap::Butt });
            CGContext::set_line_join(Some(ctx), CGLineJoin::Round);
            CGContext::stroke_path(Some(ctx));
        }
        Paint::Fill { even_odd } => {
            CGContext::set_fill_color_with_color(Some(ctx), Some(&cg_rgb8(op.color, 1.0)));
            if even_odd {
                CGContext::eo_fill_path(Some(ctx));
            } else {
                CGContext::fill_path(Some(ctx));
            }
        }
    }
    CGContext::restore_g_state(Some(ctx));
}

fn draw_text(ctx: &CGContext, op: &TextOp, scale: f64) {
    use objc2_app_kit::{
        NSFontAttributeName, NSForegroundColorAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
        NSShadow, NSShadowAttributeName, NSTextAlignment,
    };
    let font = annotation_font(op.font_size);
    let color = crate::util::ns_rgb8(op.color);
    let para = NSMutableParagraphStyle::new();
    para.setAlignment(if op.centered { NSTextAlignment::Center } else { NSTextAlignment::Left });
    let shadow = NSShadow::new();
    // NSShadow lowers to CGContextSetShadow: device space, like draw_shape.
    shadow.setShadowBlurRadius(SHADOW_BLUR * scale);
    shadow.setShadowOffset(CGSize::new(0.0, -SHADOW_DY * scale));
    shadow.setShadowColor(Some(&objc2_app_kit::NSColor::colorWithWhite_alpha(0.0, f64::from(SHADOW_OPACITY))));
    // SAFETY: the attribute-name statics are valid, values have the
    // documented types.
    let (keys, values): (Vec<&NSString>, Vec<&AnyObject>) = unsafe {
        let mut k: Vec<&NSString> =
            vec![NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName];
        let mut v: Vec<&AnyObject> = vec![&*font, &*color, &*para];
        if op.shadow {
            k.push(NSShadowAttributeName);
            v.push(&*shadow);
        }
        (k, v)
    };
    let attrs = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &values);
    let mut rect = op.rect;
    if op.centered {
        let line = crate::util::text_size("8", &font).1;
        rect = Rect::new(op.rect.min_x(), op.rect.mid().y - line / 2.0, op.rect.width(), line);
    } else {
        rect = Rect::new(rect.min_x(), rect.min_y(), rect.width() + 2.0, rect.height() + 2.0);
    }
    // The annotation's opacity applies to glyphs and their shadow alike, as
    // the live layer's `opacity` does.
    CGContext::save_g_state(Some(ctx));
    CGContext::set_alpha(Some(ctx), op.alpha);
    let gc = NSGraphicsContext::graphicsContextWithCGContext_flipped(ctx, true);
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&gc));
    // SAFETY: attributes dictionary built above.
    unsafe { NSString::from_str(&op.text).drawInRect_withAttributes(cg_rect(rect), Some(&attrs)) };
    NSGraphicsContext::restoreGraphicsState_class();
    CGContext::restore_g_state(Some(ctx));
}

/// Flatten the document: base image + annotations, cropped, at the
/// capture's native `scale`. Main thread only (AppKit text drawing).
pub fn flatten(
    base: &CGImage,
    scale: f64,
    doc: &Document,
    redactions: &mut RedactionCache,
) -> Option<CFRetained<CGImage>> {
    let visible = doc.visible_rect();
    let (w, h) = visible.pixel_size(scale);
    if w == 0 || h == 0 {
        return None;
    }
    let space = CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
        .or_else(CGColorSpace::new_device_rgb)?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: null data lets CG allocate; 8 bpc, BGRA, row bytes computed by CG.
    let ctx = unsafe { CGBitmapContextCreate(std::ptr::null_mut(), w as usize, h as usize, 8, 0, Some(&space), info) }?;
    CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::High);
    // Doc points, y down, origin at the crop's corner.
    CGContext::translate_ctm(Some(&ctx), 0.0, f64::from(h));
    CGContext::scale_ctm(Some(&ctx), scale, -scale);
    CGContext::translate_ctm(Some(&ctx), -visible.min_x(), -visible.min_y());
    let canvas = doc.canvas();
    // CGContextDrawImage draws upright in a y-up space; flip locally.
    let draw_upright = |img: &CGImage, r: Rect| {
        CGContext::save_g_state(Some(&ctx));
        CGContext::translate_ctm(Some(&ctx), r.min_x(), r.max_y());
        CGContext::scale_ctm(Some(&ctx), 1.0, -1.0);
        CGContext::draw_image(Some(&ctx), cg_rect(Rect::new(0.0, 0.0, r.width(), r.height())), Some(img));
        CGContext::restore_g_state(Some(&ctx));
    };
    draw_upright(base, canvas);
    for (_, ops) in doc.render_all() {
        for op in &ops {
            match op {
                RenderOp::Shape(s) => draw_shape(&ctx, s, scale),
                RenderOp::Text(t) => draw_text(&ctx, t, scale),
                RenderOp::Redact { rect, mode, amount } => {
                    if let Some(img) = redactions.get(base, scale, *rect, *mode, *amount) {
                        let clipped = rect.intersection(&canvas).unwrap_or(*rect);
                        draw_upright(&img, clipped);
                    }
                }
            }
        }
    }
    CGBitmapContextCreateImage(Some(&ctx))
}
