//! Tutorial images: a keyframe just after each click
//! (`AVAssetImageGenerator`, zero time tolerance), a ring drawn at the
//! click (Core Graphics), cropped around it and saved as PNG into
//! `<stem>.tutorial/`, plus an OCR hint read from a strip around the click.
//!
//! Everything blocks: call [`render_steps`] off the main thread.

use std::path::Path;

use lumora_core::geometry::{Point, Rect, Size};
use lumora_core::tutorial::{crop_rect, image_dir, image_file_name, ocr_hint, ocr_rect, Tutorial};
use objc2::rc::Retained;
use objc2_av_foundation::{AVAsset, AVAssetImageGenerator};
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo,
};
use objc2_core_media::{kCMTimeZero, CMTime};

use crate::content::rect_to_cg;
use crate::image::{pixel_size, write_png};
use crate::CaptureError;

/// Smallest crop width, in points (×scale for pixels).
const MIN_CROP_POINTS: f64 = 640.0;
/// Ring radius and line width, in points.
const RING_RADIUS: f64 = 22.0;
const RING_LINE: f64 = 3.5;

/// Exact-time frame extraction from one movie.
pub struct FrameGrabber {
    generator: Retained<AVAssetImageGenerator>,
}

impl std::fmt::Debug for FrameGrabber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FrameGrabber")
    }
}

impl FrameGrabber {
    pub fn open(movie: &Path) -> Result<FrameGrabber, CaptureError> {
        let url =
            crate::clipboard::file_url(movie).ok_or_else(|| CaptureError::Framework("invalid movie path".into()))?;
        // SAFETY: plain property setters on a fresh generator for a local file.
        let generator = unsafe {
            let asset = AVAsset::assetWithURL(&url);
            let generator = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
            generator.setAppliesPreferredTrackTransform(true);
            generator.setRequestedTimeToleranceBefore(kCMTimeZero);
            generator.setRequestedTimeToleranceAfter(kCMTimeZero);
            generator
        };
        Ok(FrameGrabber { generator })
    }

    /// The frame shown at `seconds` (full size).
    pub fn frame_at(&self, seconds: f64) -> Result<CFRetained<CGImage>, CaptureError> {
        // SAFETY: CMTime construction has no preconditions.
        let time = unsafe { CMTime::with_seconds(seconds.max(0.0), 600) };
        // Blocks until AVFoundation hands the frame over (we are off main).
        crate::av_sync::image_at(&self.generator, time)
    }
}

/// `image` with a highlight ring centred on `center` (pixels, y down).
pub fn draw_click_ring(image: &CGImage, center: Point, scale: f64) -> Result<CFRetained<CGImage>, CaptureError> {
    let (w, h) = pixel_size(image);
    if w == 0 || h == 0 {
        return Err(CaptureError::Empty);
    }
    let space = CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
        .or_else(CGColorSpace::new_device_rgb)
        .ok_or_else(|| CaptureError::Framework("no colour space".into()))?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: null data lets CG allocate; 8 bpc BGRA, CG computes row bytes.
    let ctx = unsafe { CGBitmapContextCreate(std::ptr::null_mut(), w, h, 8, 0, Some(&space), info) }
        .ok_or_else(|| CaptureError::Framework("could not create bitmap context".into()))?;
    let (wf, hf) = (w as f64, h as f64);
    CGContext::draw_image(Some(&ctx), rect_to_cg(Rect::new(0.0, 0.0, wf, hf)), Some(image));

    let s = scale.max(1.0);
    let r = RING_RADIUS * s;
    // CG is y-up.
    let ring = Rect::new(center.x - r, hf - center.y - r, 2.0 * r, 2.0 * r);
    let accent = CGColor::new_srgb(1.0, 0.62, 0.04, 1.0); // warm orange, readable on light and dark UI
    let fill = CGColor::new_srgb(1.0, 0.62, 0.04, 0.18);
    let halo = CGColor::new_srgb(1.0, 1.0, 1.0, 0.9);
    let shadow = CGColor::new_srgb(0.0, 0.0, 0.0, 0.35);

    CGContext::save_g_state(Some(&ctx));
    CGContext::set_shadow_with_color(Some(&ctx), CGSize::new(0.0, -s), 6.0 * s, Some(&shadow));
    CGContext::set_fill_color_with_color(Some(&ctx), Some(&fill));
    CGContext::fill_ellipse_in_rect(Some(&ctx), rect_to_cg(ring));
    CGContext::restore_g_state(Some(&ctx));
    // White halo under the coloured ring keeps it visible on any background.
    CGContext::set_line_width(Some(&ctx), (RING_LINE + 2.0) * s);
    CGContext::set_stroke_color_with_color(Some(&ctx), Some(&halo));
    CGContext::stroke_ellipse_in_rect(Some(&ctx), rect_to_cg(ring));
    CGContext::set_line_width(Some(&ctx), RING_LINE * s);
    CGContext::set_stroke_color_with_color(Some(&ctx), Some(&accent));
    CGContext::stroke_ellipse_in_rect(Some(&ctx), rect_to_cg(ring));
    // Centre dot.
    let d = 3.0 * s;
    CGContext::set_fill_color_with_color(Some(&ctx), Some(&accent));
    CGContext::fill_ellipse_in_rect(
        Some(&ctx),
        rect_to_cg(Rect::new(center.x - d, hf - center.y - d, 2.0 * d, 2.0 * d)),
    );
    CGBitmapContextCreateImage(Some(&ctx)).ok_or(CaptureError::Empty)
}

fn sub_image(image: &CGImage, r: Rect) -> Option<CFRetained<CGImage>> {
    if r.is_empty() {
        return None;
    }
    CGImage::with_image_in_rect(Some(image), rect_to_cg(r.integral(1.0)))
}

/// Extract, annotate and save every step's image into
/// [`image_dir`]`(movie)`, and fill each step's `image`, `crop` and
/// `ocr_hint`. A step whose frame cannot be read keeps no image.
/// `progress(done, total)` is called after each step.
pub fn render_steps(
    movie: &Path,
    tutorial: &mut Tutorial,
    mut progress: impl FnMut(usize, usize),
) -> Result<(), CaptureError> {
    let dir = image_dir(movie);
    let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    std::fs::create_dir_all(&dir)
        .map_err(|e| CaptureError::Write { path: dir.display().to_string(), reason: e.to_string() })?;
    // Old step images from an earlier run would be confusing next to new ones.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("step-") && name.ends_with(".png") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let grabber = FrameGrabber::open(movie)?;
    let scale = if tutorial.scale > 0.0 { tutorial.scale } else { 1.0 };
    let total = tutorial.steps.len();
    let mut written = 0;
    for (i, step) in tutorial.steps.iter_mut().enumerate() {
        let frame = match grabber.frame_at(step.frame_time) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("tutorial: no frame at {:.2}s: {e}", step.frame_time);
                progress(i + 1, total);
                continue;
            }
        };
        let (fw, fh) = pixel_size(&frame);
        let size = Size::new(fw as f64, fh as f64);
        // The log is in encoded-video pixels; follow the frame if it differs.
        let k = if tutorial.video_width > 0 { size.width / f64::from(tutorial.video_width) } else { 1.0 };
        let click = Point::new(step.click[0] * k, step.click[1] * k);
        let s = scale * k;

        if step.ocr_hint.is_none() {
            let region = ocr_rect(click, size, s);
            if let Some(strip) = sub_image(&frame, region) {
                match crate::ocr::recognize_boxes_sync(&strip) {
                    Ok(boxes) => {
                        let local = Point::new(click.x - region.min_x(), click.y - region.min_y());
                        step.ocr_hint = ocr_hint(&boxes, local, 14.0 * s);
                    }
                    Err(e) => log::debug!("tutorial: OCR failed: {e}"),
                }
            }
        }

        let crop = crop_rect(click, size, MIN_CROP_POINTS * s);
        let Some(piece) = sub_image(&frame, crop) else {
            progress(i + 1, total);
            continue;
        };
        let ringed = draw_click_ring(&piece, Point::new(click.x - crop.min_x(), click.y - crop.min_y()), s)?;
        let file = image_file_name(step.index);
        write_png(&ringed, &dir.join(&file), s)?;
        step.image = Some(format!("{dir_name}/{file}"));
        step.crop = Some([crop.min_x(), crop.min_y(), crop.width(), crop.height()]);
        written += 1;
        progress(i + 1, total);
    }
    if written == 0 && total > 0 {
        return Err(CaptureError::Framework("could not read any frame from the movie".into()));
    }
    Ok(())
}
