//! Core Graphics renderer for beautified captures: background, drop
//! shadow, rounded frame, optional window chrome and the capture itself on
//! one canvas, laid out by [`lumora_core::beautify::layout_at_scale`].
//!
//! The layout is y-down; the bitmap context is left y-up (no flipping CTM)
//! so `CGContextDrawImage` keeps images upright and shadow offsets — which
//! live in the context's base space and ignore the CTM — mean what they
//! say. Every rect goes through [`rect_y_up`] before it reaches CG.

use std::path::Path;

use lumora_core::beautify::{
    aspect_fill_rect, gradient_endpoints, layout_at_scale, point_y_up, rect_y_up, Background, BeautifyStyle, Preset,
    Rgba, WindowChrome,
};
use lumora_core::geometry::{Point, Rect, Size};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGSize, CFURL};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGContext, CGGradient,
    CGGradientDrawingOptions, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGInterpolationQuality, CGPath,
};
use objc2_image_io::CGImageSource;

use crate::content::rect_to_cg;
use crate::CaptureError;

/// Draw `image` (pixels; `scale` pixels per point) beautified with `style`
/// and return the new image. The style is validated first; `enabled` is not
/// consulted (callers decide whether to beautify).
pub fn render_beautified(
    image: &CGImage,
    style: &BeautifyStyle,
    scale: f64,
) -> Result<CFRetained<CGImage>, CaptureError> {
    let style = style.validated();
    let k = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (iw, ih) = crate::image::pixel_size(image);
    if iw == 0 || ih == 0 {
        return Err(CaptureError::Empty);
    }
    let layout = layout_at_scale(Size::new(iw as f64, ih as f64), &style, k);
    let (w, h) = layout.pixel_size();
    if w == 0 || h == 0 {
        return Err(CaptureError::Empty);
    }
    let ch = f64::from(h);

    let space = CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
        .or_else(CGColorSpace::new_device_rgb)
        .ok_or_else(|| CaptureError::Framework("no colour space".into()))?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: null data lets CG allocate; 8 bpc BGRA premultiplied, CG
    // computes row bytes.
    let ctx = unsafe { CGBitmapContextCreate(std::ptr::null_mut(), w as usize, h as usize, 8, 0, Some(&space), info) }
        .ok_or_else(|| CaptureError::Framework("could not create bitmap context".into()))?;
    let c = Some(&*ctx);
    CGContext::set_interpolation_quality(c, CGInterpolationQuality::High);
    CGContext::set_should_antialias(c, true);

    // 1. Background over the whole canvas (a fresh context is transparent).
    let canvas = Rect::new(0.0, 0.0, f64::from(w), ch);
    draw_background(&ctx, &space, &style.background, canvas, ch);

    let frame_up = rect_y_up(layout.frame, ch);
    let radius = layout.corner_radius;
    // SAFETY: finite rect; radius ≤ half the shorter edge (layout clamps it);
    // null transform = identity.
    let frame_path = unsafe { CGPath::with_rounded_rect(rect_to_cg(frame_up), radius, radius, std::ptr::null()) };

    // 2. Shadow: fill the rounded frame with a shadow set. The fill itself
    // is covered by the content below (opaque frame colour so the shadow
    // has a solid caster even for transparent captures).
    if let Some(sh) = style.shadow.cg_params(k) {
        CGContext::save_g_state(c);
        let black = CGColor::new_srgb(0.0, 0.0, 0.0, sh.alpha);
        CGContext::set_shadow_with_color(c, CGSize::new(sh.dx, sh.dy), sh.blur, Some(&black));
        CGContext::add_path(c, Some(&frame_path));
        set_fill(&ctx, frame_fill(&style));
        CGContext::fill_path(c);
        CGContext::restore_g_state(c);
    }

    // 3. Clip to the rounded frame; chrome bar + capture inside it.
    CGContext::save_g_state(c);
    CGContext::add_path(c, Some(&frame_path));
    CGContext::clip(c);
    if let (Some(bar), Some(color)) = (layout.chrome_bar, style.chrome.bar_color()) {
        set_fill(&ctx, color);
        CGContext::fill_rect(c, rect_to_cg(rect_y_up(bar, ch)));
        if let Some((centres, d)) = layout.traffic_lights(k) {
            for (p, color) in centres.iter().zip(WindowChrome::LIGHT_COLORS) {
                let p = point_y_up(*p, ch);
                set_fill(&ctx, color);
                CGContext::fill_ellipse_in_rect(c, rect_to_cg(Rect::new(p.x - d / 2.0, p.y - d / 2.0, d, d)));
            }
        }
    }
    CGContext::draw_image(c, rect_to_cg(rect_y_up(layout.content, ch)), Some(image));
    CGContext::restore_g_state(c);

    CGBitmapContextCreateImage(c).ok_or(CaptureError::Empty)
}

/// What fills the frame under the capture (seen only through transparent
/// capture pixels, and as the shadow caster): the chrome bar colour when
/// chrome is on, else white.
fn frame_fill(style: &BeautifyStyle) -> Rgba {
    style.chrome.bar_color().unwrap_or(Rgba::WHITE)
}

fn set_fill(ctx: &CGContext, color: Rgba) {
    let [r, g, b, a] = color.components();
    CGContext::set_rgb_fill_color(Some(ctx), r, g, b, a);
}

fn draw_background(ctx: &CGContext, space: &CGColorSpace, bg: &Background, canvas: Rect, ch: f64) {
    match bg {
        Background::None => {}
        Background::Solid(color) => {
            set_fill(ctx, *color);
            CGContext::fill_rect(Some(ctx), rect_to_cg(canvas));
        }
        Background::LinearGradient { from, to, angle_deg } => {
            draw_gradient(ctx, space, *from, *to, *angle_deg, canvas, ch);
        }
        Background::Image(path) => {
            if let Some(img) = load_image(path) {
                let (iw, ih) = crate::image::pixel_size(&img);
                let dest = aspect_fill_rect(Size::new(iw as f64, ih as f64), canvas);
                CGContext::save_g_state(Some(ctx));
                CGContext::clip_to_rect(Some(ctx), rect_to_cg(canvas));
                CGContext::draw_image(Some(ctx), rect_to_cg(rect_y_up(dest, ch)), Some(&img));
                CGContext::restore_g_state(Some(ctx));
            } else {
                log::warn!("beautify: cannot load background image {}; using default preset", path.display());
                draw_background(ctx, space, &Preset::DEFAULT.background(), canvas, ch);
            }
        }
    }
}

fn draw_gradient(ctx: &CGContext, space: &CGColorSpace, from: Rgba, to: Rgba, angle: f32, canvas: Rect, ch: f64) {
    let comps: [CGFloat; 8] = {
        let (a, b) = (from.components(), to.components());
        [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]]
    };
    let locations: [CGFloat; 2] = [0.0, 1.0];
    // SAFETY: 2 colours × 4 components in an RGB space, 2 locations.
    let Some(gradient) =
        (unsafe { CGGradient::with_color_components(Some(space), comps.as_ptr(), locations.as_ptr(), 2) })
    else {
        set_fill(ctx, from);
        CGContext::fill_rect(Some(ctx), rect_to_cg(canvas));
        return;
    };
    let (s, e) = gradient_endpoints(canvas, angle);
    let cg = |p: Point| {
        let p = point_y_up(p, ch);
        CGPoint::new(p.x, p.y)
    };
    let opts = CGGradientDrawingOptions::DrawsBeforeStartLocation | CGGradientDrawingOptions::DrawsAfterEndLocation;
    CGContext::draw_linear_gradient(Some(ctx), Some(&gradient), cg(s), cg(e), opts);
}

/// Decode the first image of the file at `path` with ImageIO.
pub fn load_image(path: &Path) -> Option<CFRetained<CGImage>> {
    if !path.is_file() {
        return None;
    }
    let url = CFURL::from_file_path(path)?;
    // SAFETY: valid file URL; options may be NULL.
    let source = unsafe { CGImageSource::with_url(&url, None) }?;
    // SAFETY: index 0 of a valid source; options may be NULL.
    unsafe { source.image_at_index(0, None) }
}

/// [`render_beautified`] when `style.enabled`, otherwise the image as-is.
/// On a render failure the plain image is returned (and the error logged),
/// so export never fails because of beautify.
pub fn beautify_for_export(image: CFRetained<CGImage>, style: &BeautifyStyle, scale: f64) -> CFRetained<CGImage> {
    if !style.enabled {
        return image;
    }
    match render_beautified(&image, style, scale) {
        Ok(out) => out,
        Err(e) => {
            log::error!("beautify failed: {e}");
            image
        }
    }
}
