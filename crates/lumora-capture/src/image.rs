//! Image utilities: cropping, raw pixel access for the loupe, and PNG
//! encoding with Retina DPI metadata.

use std::path::Path;

use lumora_core::color::Rgb8;
use lumora_core::geometry::Rect;
use objc2_core_foundation::{CFData, CFDictionary, CFMutableData, CFNumber, CFRetained, CFString, CFType, CFURL};
use objc2_core_graphics::{CGDataProvider, CGImage, CGImageByteOrderInfo};
use objc2_image_io::{kCGImagePropertyDPIHeight, kCGImagePropertyDPIWidth, CGImageDestination, CGImageSource};

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

/// 8-bit premultiplied BGRA bitmap context of `width`×`height` (sRGB) with a
/// buffer CG allocates.
fn bgra_context(width: usize, height: usize) -> Result<CFRetained<objc2_core_graphics::CGContext>, CaptureError> {
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGImageAlphaInfo};
    let space = CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
        .or_else(CGColorSpace::new_device_rgb)
        .ok_or_else(|| CaptureError::Framework("no colour space".into()))?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: null data lets CG allocate; CG computes the row bytes.
    unsafe { CGBitmapContextCreate(std::ptr::null_mut(), width, height, 8, 0, Some(&space), info) }
        .ok_or_else(|| CaptureError::Framework("could not create bitmap context".into()))
}

/// The image's pixels as tightly packed 8-bit BGRA rows (scrolling capture
/// compares and stitches these). `None` for an empty image.
pub fn bgra_pixels(image: &CGImage) -> Option<(usize, usize, Vec<u8>)> {
    use objc2_core_graphics::{CGBitmapContextGetBytesPerRow, CGBitmapContextGetData, CGContext};
    let (w, h) = pixel_size(image);
    if w == 0 || h == 0 {
        return None;
    }
    let ctx = bgra_context(w, h).ok()?;
    CGContext::draw_image(Some(&ctx), rect_to_cg(Rect::new(0.0, 0.0, w as f64, h as f64)), Some(image));
    let base = CGBitmapContextGetData(Some(&ctx)) as *const u8;
    let stride = CGBitmapContextGetBytesPerRow(Some(&ctx));
    if base.is_null() || stride < w * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        // SAFETY: the context owns `h × stride` bytes at `base`; each row
        // slice stays inside it. Bitmap rows run top to bottom in memory.
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(base.add(y * stride), w * 4) });
    }
    Some((w, h, out))
}

/// An image from tightly packed 8-bit premultiplied BGRA rows (the
/// [`bgra_pixels`] layout).
pub fn from_bgra(width: usize, height: usize, data: &[u8]) -> Result<CFRetained<CGImage>, CaptureError> {
    use objc2_core_graphics::{CGBitmapContextCreateImage, CGBitmapContextGetBytesPerRow, CGBitmapContextGetData};
    if width == 0 || height == 0 || data.len() != width * height * 4 {
        return Err(CaptureError::Empty);
    }
    // CG owns the buffer, so the image's copy-on-write stays valid.
    let ctx = bgra_context(width, height)?;
    let base = CGBitmapContextGetData(Some(&ctx)) as *mut u8;
    let stride = CGBitmapContextGetBytesPerRow(Some(&ctx));
    if base.is_null() || stride < width * 4 {
        return Err(CaptureError::Empty);
    }
    for (y, row) in data.chunks_exact(width * 4).enumerate() {
        // SAFETY: row `y` of the context's `height × stride` buffer.
        unsafe { std::ptr::copy_nonoverlapping(row.as_ptr(), base.add(y * stride), width * 4) };
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
    add_png_image(&dest, image, scale);
    if unsafe { dest.finalize() } {
        Ok(())
    } else {
        Err(err("encoding failed"))
    }
}

/// Add `image` to `dest` with DPI = `72 × scale`.
fn add_png_image(dest: &CGImageDestination, image: &CGImage, scale: f64) {
    let dpi = CFNumber::new_f64(72.0 * scale.max(1.0));
    // SAFETY: the ImageIO property keys are valid static CFStrings.
    let keys: [&CFString; 2] = unsafe { [kCGImagePropertyDPIWidth, kCGImagePropertyDPIHeight] };
    let values: [&CFType; 2] = [&dpi, &dpi];
    let props = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: `props` is a CFString → CFNumber dictionary as ImageIO expects.
    unsafe { dest.add_image(image, Some(props.as_opaque())) };
}

/// Encode `image` as PNG in memory (same DPI metadata as [`write_png`]);
/// used for the original capture inside `.lumora` documents.
pub fn png_bytes(image: &CGImage, scale: f64) -> Result<Vec<u8>, CaptureError> {
    let err = || CaptureError::Framework("PNG encoding failed".into());
    let data = CFMutableData::new(None, 0).ok_or_else(err)?;
    let png = CFString::from_static_str("public.png");
    // SAFETY: valid mutable data and UTI; options may be NULL.
    let dest = unsafe { CGImageDestination::with_data(&data, &png, 1, None) }.ok_or_else(err)?;
    add_png_image(&dest, image, scale);
    // SAFETY: one image was added.
    if !unsafe { dest.finalize() } {
        return Err(err());
    }
    drop(dest);
    Ok(data.to_vec())
}

/// Decode PNG (or any ImageIO-readable) bytes into an image.
pub fn decode_image(bytes: &[u8]) -> Option<CFRetained<CGImage>> {
    let data = CFData::from_bytes(bytes);
    // SAFETY: valid data; options may be NULL.
    let source = unsafe { CGImageSource::with_data(&data, None) }?;
    // SAFETY: index 0 of a valid source; options may be NULL.
    unsafe { source.image_at_index(0, None) }
}

/// ImageIO options for a thumbnail at most `max_px` on its longer side
/// (always decoded from the full image, EXIF orientation applied).
fn thumbnail_options(max_px: u32) -> CFRetained<CFDictionary<CFString, CFType>> {
    use objc2_core_foundation::CFBoolean;
    use objc2_image_io::{
        kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform,
        kCGImageSourceThumbnailMaxPixelSize,
    };
    let size = CFNumber::new_i32(max_px.clamp(16, 8192) as i32);
    let yes: &CFType = CFBoolean::new(true);
    // SAFETY: the ImageIO option keys are valid static CFStrings.
    let keys: [&CFString; 3] = unsafe {
        [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceThumbnailMaxPixelSize,
            kCGImageSourceCreateThumbnailWithTransform,
        ]
    };
    let values: [&CFType; 3] = [yes, &size, yes];
    CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
}

/// A downscaled copy of the image file at `path` (PNG, JPEG, …), at most
/// `max_px` on its longer side, without decoding the full image first.
pub fn thumbnail_file(path: &Path, max_px: u32) -> Option<CFRetained<CGImage>> {
    let url = CFURL::from_file_path(path)?;
    // SAFETY: valid URL; options may be NULL.
    let source = unsafe { CGImageSource::with_url(&url, None) }?;
    let opts = thumbnail_options(max_px);
    // SAFETY: index 0 of a valid source; the options dictionary holds the
    // documented key/value types.
    unsafe { source.thumbnail_at_index(0, Some(opts.as_opaque())) }
}

/// Like [`thumbnail_file`], from encoded bytes (a `.lumora`'s PNG).
pub fn thumbnail_data(bytes: &[u8], max_px: u32) -> Option<CFRetained<CGImage>> {
    let data = CFData::from_bytes(bytes);
    // SAFETY: valid data; options may be NULL.
    let source = unsafe { CGImageSource::with_data(&data, None) }?;
    let opts = thumbnail_options(max_px);
    // SAFETY: as in `thumbnail_file`.
    unsafe { source.thumbnail_at_index(0, Some(opts.as_opaque())) }
}

/// Decode the image file at `path` with its backing scale, read from the
/// DPI Lumora writes (`72 × scale`); 1× when the file has none.
pub fn read_file(path: &Path) -> Option<(CFRetained<CGImage>, f64)> {
    let url = CFURL::from_file_path(path)?;
    // SAFETY: valid URL; options may be NULL.
    let source = unsafe { CGImageSource::with_url(&url, None) }?;
    // SAFETY: index 0 of a valid source; options may be NULL.
    let image = unsafe { source.image_at_index(0, None) }?;
    // SAFETY: as above.
    let dpi = unsafe { source.properties_at_index(0, None) }.and_then(|props| {
        // SAFETY: image properties are a CFString-keyed dictionary.
        let props = unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(props) };
        // SAFETY: valid static key.
        let v = props.get(unsafe { kCGImagePropertyDPIWidth })?;
        v.downcast_ref::<CFNumber>()?.as_f64()
    });
    Some((image, lumora_core::history::scale_for_dpi(dpi)))
}
