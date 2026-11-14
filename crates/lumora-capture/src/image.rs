//! Image utilities: cropping, raw pixel access for the loupe, and PNG
//! encoding with Retina DPI metadata.

use std::path::Path;

use lumora_core::color::Rgb8;
use lumora_core::geometry::Rect;
use objc2_core_foundation::{CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL};
use objc2_core_graphics::{CGDataProvider, CGImage, CGImageByteOrderInfo};
use objc2_image_io::{kCGImagePropertyDPIHeight, kCGImagePropertyDPIWidth, CGImageDestination};

use crate::content::rect_to_cg;
use crate::CaptureError;

/// Pixel dimensions of an image.
pub fn pixel_size(image: &CGImage) -> (usize, usize) {
    (CGImage::width(Some(image)), CGImage::height(Some(image)))
}

/// Crop `image` (which shows `image_frame` in global points) to `rect`
/// (global points). The rect is clipped to the image and aligned outward to
/// whole pixels so the result is never blurry.
pub fn crop(image: &CGImage, image_frame: Rect, rect: Rect) -> Result<CFRetained<CGImage>, CaptureError> {
    let (pw, _) = pixel_size(image);
    let scale = if image_frame.width() > 0.0 { pw as f64 / image_frame.width() } else { 1.0 };
    let clipped = rect.intersection(&image_frame).ok_or(CaptureError::Empty)?;
    let local = clipped.relative_to(&image_frame);
    let px = Rect::new(local.min_x() * scale, local.min_y() * scale, local.width() * scale, local.height() * scale)
        .integral(1.0);
    CGImage::with_image_in_rect(Some(image), rect_to_cg(px)).ok_or(CaptureError::Empty)
}

/// One display's contribution to a stitched capture.
pub struct StitchPart<'a> {
    pub image: &'a CGImage,
    /// Global frame (points) the image shows.
    pub frame: Rect,
    /// Region to take (global points).
    pub area: Rect,
    /// Destination in output pixels (y down).
    pub dest: Rect,
}

/// Compose a `width`×`height` image from parts of several display frames
/// (a selection spanning displays). Uncovered pixels are transparent.
pub fn stitch(width: u32, height: u32, parts: &[StitchPart<'_>]) -> Result<CFRetained<CGImage>, CaptureError> {
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext, CGImageAlphaInfo,
        CGInterpolationQuality,
    };
    if width == 0 || height == 0 {
        return Err(CaptureError::Empty);
    }
    let space = CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
        .or_else(CGColorSpace::new_device_rgb)
        .ok_or_else(|| CaptureError::Framework("no colour space".into()))?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: null data lets CG allocate; 8 bpc BGRA, CG computes row bytes.
    let ctx = unsafe {
        CGBitmapContextCreate(std::ptr::null_mut(), width as usize, height as usize, 8, 0, Some(&space), info)
    }
    .ok_or_else(|| CaptureError::Framework("could not create bitmap context".into()))?;
    CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::High);
    let h = f64::from(height);
    for p in parts {
        let piece = crop(p.image, p.frame, p.area)?;
        // CG draws in a y-up space.
        let r = Rect::new(p.dest.min_x(), h - p.dest.max_y(), p.dest.width(), p.dest.height());
        CGContext::draw_image(Some(&ctx), rect_to_cg(r), Some(&piece));
    }
    CGBitmapContextCreateImage(Some(&ctx)).ok_or(CaptureError::Empty)
}

/// A CPU copy of an image's pixels, used to read colours under the loupe.
pub struct PixelBuffer {
    data: CFRetained<CFData>,
    width: usize,
    height: usize,
    bytes_per_row: usize,
    bytes_per_pixel: usize,
    /// True for BGRA byte order (little-endian 32-bit, alpha first).
    bgra: bool,
}

impl std::fmt::Debug for PixelBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PixelBuffer").field("width", &self.width).field("height", &self.height).finish()
    }
}

impl PixelBuffer {
    /// Copy the pixels out of `image`. Returns `None` for formats other than
    /// 8-bit-per-channel RGBA/BGRA (ScreenCaptureKit delivers BGRA).
    pub fn from_image(image: &CGImage) -> Option<PixelBuffer> {
        if CGImage::bits_per_component(Some(image)) != 8 {
            return None;
        }
        let bytes_per_pixel = CGImage::bits_per_pixel(Some(image)) / 8;
        if bytes_per_pixel < 3 {
            return None;
        }
        let provider = CGImage::data_provider(Some(image))?;
        let data = CGDataProvider::data(Some(&provider))?;
        let bgra = CGImage::byte_order_info(Some(image)) == CGImageByteOrderInfo::Order32Little;
        Some(PixelBuffer {
            data,
            width: CGImage::width(Some(image)),
            height: CGImage::height(Some(image)),
            bytes_per_row: CGImage::bytes_per_row(Some(image)),
            bytes_per_pixel,
            bgra,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }

    /// Colour at pixel `(x, y)`, or `None` out of bounds.
    pub fn pixel(&self, x: usize, y: usize) -> Option<Rgb8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let len = self.data.length() as usize;
        let offset = y * self.bytes_per_row + x * self.bytes_per_pixel;
        if offset + 4 > len {
            return None;
        }
        // SAFETY: `byte_ptr` points at `len` bytes owned by `self.data`,
        // and the slice stays within bounds (checked above).
        let bytes = unsafe { std::slice::from_raw_parts(self.data.byte_ptr().add(offset), 4) };
        Rgb8::from_pixel(bytes, self.bgra)
    }
}

/// Encode `image` as PNG at `path`. `scale` is the capture's backing scale;
/// the file's DPI is set to `72 × scale` so a 2× capture displays at its
/// point size in Preview, Keynote, browsers, etc.
pub fn write_png(image: &CGImage, path: &Path, scale: f64) -> Result<(), CaptureError> {
    let err = |reason: &str| CaptureError::Write { path: path.display().to_string(), reason: reason.to_string() };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| err(&e.to_string()))?;
    }
    let url = CFURL::from_file_path(path).ok_or_else(|| err("invalid path"))?;
    let png = CFString::from_static_str("public.png");
    // SAFETY: valid URL and UTI; options may be NULL.
    let dest = unsafe { CGImageDestination::with_url(&url, &png, 1, None) }.ok_or_else(|| err("cannot create file"))?;
    let dpi = CFNumber::new_f64(72.0 * scale.max(1.0));
    // SAFETY: the ImageIO property keys are valid static CFStrings.
    let keys: [&CFString; 2] = unsafe { [kCGImagePropertyDPIWidth, kCGImagePropertyDPIHeight] };
    let values: [&CFType; 2] = [&dpi, &dpi];
    let props = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: `props` is a CFString → CFNumber dictionary as ImageIO expects.
    unsafe { dest.add_image(image, Some(props.as_opaque())) };
    if unsafe { dest.finalize() } {
        Ok(())
    } else {
        Err(err("encoding failed"))
    }
}
