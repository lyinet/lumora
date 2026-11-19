//! Trim a recording (AVAssetExportSession, passthrough) and turn a range of
//! it into an animated GIF (AVAssetImageGenerator + ImageIO).
//!
//! Everything here blocks; call it from a worker thread. Both exports check
//! a cancel flag and report progress (0…1) through a callback.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use lumora_core::clip::{GifPlan, TrimRange};
use objc2_av_foundation::{
    AVAsset, AVAssetExportPresetHighestQuality, AVAssetExportPresetPassthrough, AVAssetExportSession,
    AVAssetImageGenerator, AVFileTypeMPEG4, AVFileTypeQuickTimeMovie, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType, CGSize, CFURL};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_image_io::{
    kCGImagePropertyGIFDelayTime, kCGImagePropertyGIFDictionary, kCGImagePropertyGIFLoopCount,
    kCGImagePropertyGIFUnclampedDelayTime, CGImageDestination,
};

use crate::CaptureError;

/// Timescale for the CMTimes we build (600 divides every common frame rate).
const TIMESCALE: i32 = 600;

/// What [`movie_info`] reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovieInfo {
    pub duration: f64,
    /// Pixel size of the first video track (display orientation).
    pub width: u32,
    pub height: u32,
}

pub(crate) fn asset_for(path: &Path) -> Result<objc2::rc::Retained<AVAsset>, CaptureError> {
    let url = crate::clipboard::file_url(path).ok_or_else(|| CaptureError::Framework("invalid movie path".into()))?;
    // SAFETY: opening a local file asset.
    Ok(unsafe { AVAsset::assetWithURL(&url) })
}

/// Duration and video size of `movie`. Blocks.
pub fn movie_info(movie: &Path) -> Result<MovieInfo, CaptureError> {
    let asset = asset_for(movie)?;
    // SAFETY: a valid static media type.
    let video =
        unsafe { AVMediaTypeVideo }.ok_or_else(|| CaptureError::Framework("AVMediaTypeVideo missing".into()))?;
    let track = crate::av_sync::tracks(&asset, video)?
        .into_iter()
        .next()
        .ok_or_else(|| CaptureError::Framework("the recording has no video".into()))?;
    // SAFETY: plain property reads once the tracks are loaded.
    #[allow(deprecated)] // `loadValuesAsynchronously…` for one number isn't worth a round-trip.
    let (duration, size) = unsafe { (asset.duration().seconds(), track.naturalSize()) };
    let duration = if duration.is_finite() { duration.max(0.0) } else { 0.0 };
    Ok(MovieInfo { duration, width: size.width.abs().round() as u32, height: size.height.abs().round() as u32 })
}

/// The frame of `movie` at `time` (seconds), at most `max_side` pixels on
/// its longer side, for the trim window's and the subtitle preview's
/// frames. Blocks.
pub fn frame_at(movie: &Path, time: f64, max_side: f64) -> Option<CFRetained<objc2_core_graphics::CGImage>> {
    let asset = asset_for(movie).ok()?;
    // SAFETY: plain setters on a fresh generator; a little tolerance keeps
    // scrubbing quick (it may snap to a nearby frame).
    let (generator, at) = unsafe {
        let g = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
        g.setAppliesPreferredTrackTransform(true);
        g.setMaximumSize(CGSize::new(max_side, max_side));
        let tol = CMTime::with_seconds(0.05, TIMESCALE);
        g.setRequestedTimeToleranceBefore(tol);
        g.setRequestedTimeToleranceAfter(tol);
        (g, CMTime::with_seconds(time.max(0.0), TIMESCALE))
    };
    crate::av_sync::image_at(&generator, at).inspect_err(|e| log::warn!("preview frame at {time:.2} s: {e}")).ok()
}

fn time_range(range: TrimRange) -> CMTimeRange {
    // SAFETY: CMTime construction has no preconditions.
    unsafe {
        CMTimeRange {
            start: CMTime::with_seconds(range.start, TIMESCALE),
            duration: CMTime::with_seconds(range.len(), TIMESCALE),
        }
    }
}

/// Write `range` of `movie` to `dest` (must not exist), keeping the
/// container of `dest`'s extension (`.mp4` or `.mov`). Uses the passthrough
/// preset (no re-encode); falls back to re-encoding at the highest quality
/// when passthrough is not offered. Removes `dest` on failure or cancel.
pub fn export_trim(
    movie: &Path,
    dest: &Path,
    range: TrimRange,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), CaptureError> {
    let asset = asset_for(movie)?;
    let url = crate::clipboard::file_url(dest).ok_or_else(|| CaptureError::Framework("invalid output path".into()))?;
    let mp4 = dest.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("m4v"));
    // SAFETY: standard export session setup on a local asset; the preset
    // and file-type constants are valid statics.
    let session = unsafe {
        let file_type = if mp4 { AVFileTypeMPEG4 } else { AVFileTypeQuickTimeMovie }
            .ok_or_else(|| CaptureError::Framework("movie file type missing".into()))?;
        let session = AVAssetExportSession::exportSessionWithAsset_presetName(&asset, AVAssetExportPresetPassthrough)
            .or_else(|| {
                log::info!("trim: passthrough unavailable, re-encoding");
                AVAssetExportSession::exportSessionWithAsset_presetName(&asset, AVAssetExportPresetHighestQuality)
            })
            .ok_or_else(|| CaptureError::Framework("cannot export this recording".into()))?;
        session.setOutputURL(Some(&url));
        session.setOutputFileType(Some(file_type));
        session.setTimeRange(time_range(range));
        session
    };
    let result = crate::transcribe::run_export(&session, Some(cancel), Some(progress));
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

fn gif_err(dest: &Path, reason: &str) -> CaptureError {
    CaptureError::Write { path: dest.display().to_string(), reason: reason.to_string() }
}

/// Write `plan`'s frames of `movie` to `dest` as a GIF that loops forever.
/// Removes `dest` on failure or cancel.
pub fn export_gif(
    movie: &Path,
    dest: &Path,
    plan: &GifPlan,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), CaptureError> {
    let result = write_gif(movie, dest, plan, cancel, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

fn write_gif(
    movie: &Path,
    dest: &Path,
    plan: &GifPlan,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), CaptureError> {
    if plan.frames() == 0 {
        return Err(CaptureError::Empty);
    }
    let asset = asset_for(movie)?;
    // SAFETY: plain setters on a fresh generator for a local file. A
    // tolerance of half a frame lets AVFoundation decode fewer frames.
    let generator = unsafe {
        let g = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
        g.setAppliesPreferredTrackTransform(true);
        g.setMaximumSize(CGSize::new(plan.width as f64, plan.height as f64));
        let fps = plan.frames() as f64 / plan.delays_cs.iter().map(|&d| d as f64 / 100.0).sum::<f64>().max(0.01);
        let tol = CMTime::with_seconds(0.5 / fps.max(1.0), TIMESCALE);
        g.setRequestedTimeToleranceBefore(tol);
        g.setRequestedTimeToleranceAfter(tol);
        g
    };
    let url = CFURL::from_file_path(dest).ok_or_else(|| gif_err(dest, "invalid path"))?;
    let gif = CFString::from_static_str("com.compuserve.gif");
    // SAFETY: valid URL and UTI; options may be NULL.
    let destination = unsafe { CGImageDestination::with_url(&url, &gif, plan.frames(), None) }
        .ok_or_else(|| gif_err(dest, "cannot create file"))?;
    // Loop forever.
    let zero = CFNumber::new_i32(0);
    // SAFETY: ImageIO property keys are valid static CFStrings.
    let (gif_key, loop_key, delay_key, unclamped_key) = unsafe {
        (
            kCGImagePropertyGIFDictionary,
            kCGImagePropertyGIFLoopCount,
            kCGImagePropertyGIFDelayTime,
            kCGImagePropertyGIFUnclampedDelayTime,
        )
    };
    let file_props = nested(gif_key, &CFDictionary::<CFString, CFType>::from_slices(&[loop_key], &[&*zero]));
    // SAFETY: a CFString-keyed dictionary as ImageIO expects.
    unsafe { destination.set_properties(Some(file_props.as_opaque())) };

    let n = plan.frames();
    for (i, (&t, &delay)) in plan.times.iter().zip(&plan.delays_cs).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(CaptureError::Cancelled);
        }
        // SAFETY: CMTime construction has no preconditions.
        let time = unsafe { CMTime::with_seconds(t, TIMESCALE) };
        let image = crate::av_sync::image_at(&generator, time)?;
        let seconds = CFNumber::new_f64(delay as f64 / 100.0);
        let frame = CFDictionary::<CFString, CFType>::from_slices(&[delay_key, unclamped_key], &[&*seconds, &*seconds]);
        let props = nested(gif_key, &frame);
        // SAFETY: valid image and CFString-keyed properties.
        unsafe { destination.add_image(&image, Some(props.as_opaque())) };
        // Frames are ~90 % of the work; finalising (encoding) the rest.
        progress(0.9 * (i + 1) as f32 / n as f32);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(CaptureError::Cancelled);
    }
    // SAFETY: every frame was added; finalising writes the file.
    if unsafe { destination.finalize() } {
        progress(1.0);
        Ok(())
    } else {
        Err(gif_err(dest, "encoding failed"))
    }
}

/// `{ key: inner }`.
fn nested(key: &CFString, inner: &CFDictionary<CFString, CFType>) -> CFRetained<CFDictionary<CFString, CFType>> {
    let value: &CFType = inner.as_ref();
    CFDictionary::<CFString, CFType>::from_slices(&[key], &[value])
}
