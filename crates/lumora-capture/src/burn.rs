//! Burn subtitles into a recording: AVMutableComposition + an
//! AVMutableVideoComposition whose AVVideoCompositionCoreAnimationTool draws
//! one text layer per cue over the video, exported with
//! AVAssetExportSession (HighestQuality; audio is carried along).
//!
//! The layout (font size, wrapping, boxes, timing) comes from
//! [`lumora_core::transcript::burn`]. [`cue_overlay`] is shared with the
//! transcript window's live preview. [`export_subtitled`] and
//! [`preview_canvas`] block; call them from a worker thread.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use lumora_core::transcript::burn::{layout_cues, opacity_keyframes, BurnLayout, Cue, SubtitleStyle, BOX_ALPHA};
use lumora_core::transcript::Segment;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_app_kit::{
    NSColor, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSTextAlignment,
};
use objc2_av_foundation::{
    AVAsset, AVAssetExportPresetHighestQuality, AVAssetExportSession, AVCoreAnimationBeginTimeAtZero, AVFileTypeMPEG4,
    AVFileTypeQuickTimeMovie, AVMutableComposition, AVMutableVideoComposition, AVVideoCompositionCoreAnimationTool,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGColor;
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_foundation::{NSArray, NSAttributedString, NSAttributedStringKey, NSDictionary, NSNumber, NSString};
use objc2_quartz_core::{
    kCAAnimationDiscrete, kCAFillModeForwards, CAKeyframeAnimation, CALayer, CAMediaTiming, CATextLayer, CATransaction,
};

use crate::CaptureError;

fn cg(r: lumora_core::geometry::Rect) -> CGRect {
    CGRect::new(CGPoint::new(r.min_x(), r.min_y()), CGSize::new(r.width(), r.height()))
}

/// The styled text for a cue: white bold system font, centred, with a
/// fixed line height so CJK fallback fonts keep the box's metrics.
fn attributed(text: &str, layout: &BurnLayout) -> Retained<NSAttributedString> {
    let font = NSFont::boldSystemFontOfSize(layout.font_size);
    let color = NSColor::whiteColor();
    let para = NSMutableParagraphStyle::new();
    para.setAlignment(NSTextAlignment::Center);
    para.setMinimumLineHeight(layout.line_height);
    para.setMaximumLineHeight(layout.line_height);
    // SAFETY: AppKit attribute keys are valid static NSStrings.
    let keys: [&NSAttributedStringKey; 3] =
        unsafe { [NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName] };
    let values: [&AnyObject; 3] = [&font, &color, &para];
    let attrs = NSDictionary::<NSAttributedStringKey, AnyObject>::from_slices(&keys, &values);
    // SAFETY: a fresh attributed string from a valid string and attributes.
    unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attrs),
        )
    }
}

/// A discrete opacity animation showing `cue` only during its time.
fn visibility(cue: &Cue, duration: f64) -> Option<Retained<CAKeyframeAnimation>> {
    let (values, key_times) = opacity_keyframes(cue.start, cue.end, duration)?;
    let values: Vec<Retained<NSNumber>> = values.into_iter().map(|v| NSNumber::new_f32(v as f32)).collect();
    let key_times: Vec<Retained<NSNumber>> = key_times.into_iter().map(NSNumber::new_f64).collect();
    let values = NSArray::from_retained_slice(&values);
    let key_times = NSArray::from_retained_slice(&key_times);
    let anim = CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
    // SAFETY: an NSArray of NSNumber is a valid `values` array for a
    // scalar key path; the upcast only forgets the element type.
    unsafe {
        let any: &NSArray = &*(Retained::as_ptr(&values) as *const NSArray);
        anim.setValues(Some(any));
    }
    anim.setKeyTimes(Some(&key_times));
    // SAFETY: valid static constants.
    unsafe {
        anim.setCalculationMode(kCAAnimationDiscrete);
        anim.setFillMode(kCAFillModeForwards);
        // AVFoundation treats a beginTime of 0 as "now"; this constant is
        // its stand-in for the start of the movie.
        anim.setBeginTime(AVCoreAnimationBeginTimeAtZero);
    }
    anim.setDuration(duration);
    anim.setRemovedOnCompletion(false);
    Some(anim)
}

/// The overlay for one cue — background box and text — in a
/// `video_w`×`video_h` layer with Core Animation's bottom-left origin. This
/// is the one drawing path for subtitles: the export animates its opacity
/// over the movie and the transcript window's live preview shows it scaled
/// over a frame, so the two can't drift apart. `contents_scale` is the text
/// rasterisation scale (1.0 = one pixel per video pixel, the export's).
pub fn cue_overlay(cue: &Cue, layout: &BurnLayout, contents_scale: f64) -> Retained<CALayer> {
    let group = CALayer::layer();
    group.setFrame(CGRect::new(CGPoint::ZERO, CGSize::new(layout.video_w, layout.video_h)));
    if layout.style.background {
        let bg = CALayer::layer();
        bg.setFrame(cg(cue.box_rect));
        let black = CGColor::new_generic_gray(0.0, BOX_ALPHA);
        bg.setBackgroundColor(Some(&black));
        bg.setCornerRadius(layout.corner_radius);
        group.addSublayer(&bg);
    }
    let text = CATextLayer::layer();
    text.setFrame(cg(cue.text_rect));
    // SAFETY: CATextLayer accepts an NSAttributedString as its string.
    unsafe { text.setString(Some(&attributed(&cue.text(), layout))) };
    text.setWrapped(false);
    // SAFETY: valid static constant.
    text.setAlignmentMode(unsafe { objc2_quartz_core::kCAAlignmentCenter });
    text.setContentsScale(contents_scale);
    group.addSublayer(&text);
    group
}

/// The export's layer for one cue: [`cue_overlay`] at video pixels (1.0
/// keeps text crisp at the export size), hidden outside its time.
fn cue_layer(cue: &Cue, layout: &BurnLayout, duration: f64) -> Option<Retained<CALayer>> {
    let anim = visibility(cue, duration)?;
    let group = cue_overlay(cue, layout, 1.0);
    group.setOpacity(0.0);
    group.addAnimation_forKey(&anim, Some(&NSString::from_str("lumoraSubtitle")));
    Some(group)
}

/// What the preview needs to lay subtitles out like the export: the
/// video's render size (the size the export's video composition renders
/// at, display orientation) and duration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Canvas {
    pub width: f64,
    pub height: f64,
    pub duration: f64,
}

/// The render size the export would use for `asset` (or a composition of
/// it).
fn render_size(asset: &AVAsset) -> CGSize {
    // SAFETY: instructions, frame rate and render size from the asset.
    #[allow(deprecated)] // the completion-handler variant is macOS 15+.
    let vc = unsafe { AVMutableVideoComposition::videoCompositionWithPropertiesOfAsset(asset) };
    // SAFETY: plain property read.
    unsafe { vc.renderSize() }
}

/// `movie`'s [`Canvas`], or `None` when it has no video track (an
/// audio-only recording). Blocks; call it from a worker thread.
pub fn preview_canvas(movie: &Path) -> Result<Option<Canvas>, CaptureError> {
    let asset = crate::clip::asset_for(movie)?;
    // SAFETY: a valid static media type.
    let video = unsafe { objc2_av_foundation::AVMediaTypeVideo }
        .ok_or_else(|| CaptureError::Framework("AVMediaTypeVideo missing".into()))?;
    if crate::av_sync::tracks(&asset, video)?.is_empty() {
        return Ok(None);
    }
    // SAFETY: duration is loaded with the tracks above.
    #[allow(deprecated)]
    let duration = unsafe { asset.duration().seconds() };
    let size = render_size(&asset);
    if !duration.is_finite() || duration <= 0.0 || size.width <= 0.0 || size.height <= 0.0 {
        return Err(CaptureError::Empty);
    }
    Ok(Some(Canvas { width: size.width, height: size.height, duration }))
}

/// Write `movie` to `dest` (must not exist) with `segments` burned in.
/// `dest`'s extension picks the container (`.mp4`/`.m4v` or `.mov`).
/// Removes `dest` on failure or cancel.
pub fn export_subtitled(
    movie: &Path,
    dest: &Path,
    segments: &[Segment],
    style: SubtitleStyle,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), CaptureError> {
    let result = burn(movie, dest, segments, style, cancel, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

fn burn(
    movie: &Path,
    dest: &Path,
    segments: &[Segment],
    style: SubtitleStyle,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), CaptureError> {
    let asset = crate::clip::asset_for(movie)?;
    let url = crate::clipboard::file_url(dest).ok_or_else(|| CaptureError::Framework("invalid output path".into()))?;
    // Load the tracks synchronously before the composition reads them.
    // SAFETY: a valid static media type.
    let video = unsafe { objc2_av_foundation::AVMediaTypeVideo }
        .ok_or_else(|| CaptureError::Framework("AVMediaTypeVideo missing".into()))?;
    if crate::av_sync::tracks(&asset, video)?.is_empty() {
        return Err(CaptureError::Framework("the recording has no video".into()));
    }
    // SAFETY: duration is loaded with the tracks above.
    #[allow(deprecated)]
    let duration_cm = unsafe { asset.duration() };
    // SAFETY: plain CMTime read.
    let duration = unsafe { duration_cm.seconds() };
    if !duration.is_finite() || duration <= 0.0 {
        return Err(CaptureError::Empty);
    }

    // SAFETY: a fresh composition holding every track of the movie.
    let composition = unsafe {
        let c = AVMutableComposition::composition();
        let range = CMTimeRange { start: CMTime::with_seconds(0.0, 600), duration: duration_cm };
        #[allow(deprecated)] // the async variant is macOS 15+.
        c.insertTimeRange_ofAsset_atTime_error(range, &asset, CMTime::with_seconds(0.0, 600))
            .map_err(|e| CaptureError::Framework(format!("composition: {}", e.localizedDescription())))?;
        c
    };
    // SAFETY: instructions, frame rate and render size from the composition.
    #[allow(deprecated)] // the completion-handler variant is macOS 15+.
    let vc = unsafe { AVMutableVideoComposition::videoCompositionWithPropertiesOfAsset(&composition) };
    // SAFETY: plain property read; the same size `render_size` gives the
    // preview.
    let size = unsafe { vc.renderSize() };
    let layout = BurnLayout::new(size.width, size.height, style);
    let cues = layout_cues(segments, &layout, duration);
    if cues.is_empty() {
        return Err(CaptureError::Empty);
    }

    // Layers are built off the main thread: keep it in one explicit
    // transaction with implicit animations off.
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let frame = CGRect::new(CGPoint::ZERO, size);
    let parent = CALayer::layer();
    parent.setFrame(frame);
    parent.setGeometryFlipped(false);
    let video_layer = CALayer::layer();
    video_layer.setFrame(frame);
    parent.addSublayer(&video_layer);
    for cue in &cues {
        if let Some(layer) = cue_layer(cue, &layout, duration) {
            parent.addSublayer(&layer);
        }
    }
    CATransaction::commit();
    // SAFETY: the video layer sits in the parent's tree; neither layer is
    // used anywhere else.
    unsafe {
        let tool = AVVideoCompositionCoreAnimationTool::videoCompositionCoreAnimationToolWithPostProcessingAsVideoLayer_inLayer(
            &video_layer,
            &parent,
        );
        vc.setAnimationTool(Some(&tool));
    }

    let mp4 = dest.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("m4v"));
    // SAFETY: standard export session setup; constants are valid statics.
    let session = unsafe {
        let file_type = if mp4 { AVFileTypeMPEG4 } else { AVFileTypeQuickTimeMovie }
            .ok_or_else(|| CaptureError::Framework("movie file type missing".into()))?;
        let session =
            AVAssetExportSession::exportSessionWithAsset_presetName(&composition, AVAssetExportPresetHighestQuality)
                .ok_or_else(|| CaptureError::Framework("cannot export this recording".into()))?;
        session.setOutputURL(Some(&url));
        session.setOutputFileType(Some(file_type));
        session.setVideoComposition(Some(&vc));
        session
    };
    crate::transcribe::run_export(&session, Some(cancel), Some(progress))
}
