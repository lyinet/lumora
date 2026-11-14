//! Still captures through `SCScreenshotManager` (macOS 14+).

use std::sync::Mutex;

use block2::RcBlock;
use lumora_core::geometry::Rect;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};

use crate::content::{ns_error, rect_from_cg, rect_to_cg, ShareableContent};
use crate::CaptureError;

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShotTarget {
    /// A whole display by `CGDirectDisplayID`.
    Display(u32),
    /// A single window by `CGWindowID`, captured independently of what
    /// overlaps it.
    Window(u32),
    /// A rectangle in global top-left points; captured from the display that
    /// contains its centre and clipped to it.
    Area(Rect),
}

/// Capture options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShotOptions {
    pub show_cursor: bool,
    /// Capture at native pixel density (2× on Retina) rather than 1×.
    pub retina: bool,
    /// Include the drop shadow for window captures.
    pub window_shadow: bool,
}

impl Default for ShotOptions {
    fn default() -> Self {
        ShotOptions { show_cursor: false, retina: true, window_shadow: true }
    }
}

/// A finished still.
pub struct Shot {
    pub image: CFRetained<CGImage>,
    /// Pixels per point of `image`.
    pub scale: f64,
    /// What the image covers, in global top-left points.
    pub frame: Rect,
}

impl std::fmt::Debug for Shot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shot").field("scale", &self.scale).field("frame", &self.frame).finish_non_exhaustive()
    }
}

/// Take a screenshot. `completion` runs on an arbitrary queue.
pub fn capture(
    content: &ShareableContent,
    target: ShotTarget,
    opts: ShotOptions,
    completion: impl FnOnce(Result<Shot, CaptureError>) + Send + 'static,
) {
    let prepared = prepare(content, target, opts);
    let (filter, config, frame, scale) = match prepared {
        Ok(p) => p,
        Err(e) => return completion(Err(e)),
    };
    let slot = Mutex::new(Some(completion));
    let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let Some(cb) = slot.lock().ok().and_then(|mut s| s.take()) else { return };
        // SAFETY: `image` is null or a valid CGImage for the duration of the
        // callback; `retain` extends that.
        let result = match std::ptr::NonNull::new(image) {
            Some(ptr) => Ok(Shot { image: unsafe { CFRetained::retain(ptr) }, scale, frame }),
            None => Err(ns_error(error)),
        };
        cb(result);
    });
    // SAFETY: filter/config are fully initialised; block has the right type.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &config, Some(&block));
    }
}

type Prepared = (Retained<SCContentFilter>, Retained<SCStreamConfiguration>, Rect, f64);

fn prepare(content: &ShareableContent, target: ShotTarget, opts: ShotOptions) -> Result<Prepared, CaptureError> {
    // SAFETY (whole fn): straightforward object construction and property
    // setters on freshly created, unshared objects.
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setShowsCursor(opts.show_cursor);
        config.setCaptureResolution(if opts.retina {
            SCCaptureResolutionType::Best
        } else {
            SCCaptureResolutionType::Nominal
        });
        match target {
            ShotTarget::Display(_) | ShotTarget::Area(_) => {
                let display_info = match target {
                    ShotTarget::Area(r) => content.display_for_rect(r),
                    ShotTarget::Display(id) => content.displays().into_iter().find(|d| d.id == id),
                    ShotTarget::Window(_) => None,
                }
                .ok_or(CaptureError::TargetGone("display"))?;
                let display = content.sc_display(display_info.id).ok_or(CaptureError::TargetGone("display"))?;
                let filter = SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &content.own_windows(),
                );
                let scale = if opts.retina { display_info.scale } else { 1.0 };
                let frame = match target {
                    ShotTarget::Area(r) => {
                        let clipped = r.intersection(&display_info.frame).ok_or(CaptureError::Empty)?.integral(scale);
                        // sourceRect is in display-local points.
                        config.setSourceRect(rect_to_cg(clipped.relative_to(&display_info.frame)));
                        clipped
                    }
                    _ => display_info.frame,
                };
                let (w, h) = frame.pixel_size(scale);
                config.setWidth(w as usize);
                config.setHeight(h as usize);
                Ok((filter, config, frame, scale))
            }
            ShotTarget::Window(id) => {
                let window = content.sc_window(id).ok_or(CaptureError::TargetGone("window"))?;
                let filter = SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window);
                config.setIgnoreShadowsSingleWindow(!opts.window_shadow);
                config.setShouldBeOpaque(false);
                // infoForFilter reports the content rect (including shadow
                // when not ignored) and the point→pixel scale.
                let info = SCShareableContent::infoForFilter(&filter);
                let rect = info.contentRect();
                let scale = if opts.retina { f64::from(info.pointPixelScale()) } else { 1.0 };
                config.setWidth((rect.size.width * scale).round() as usize);
                config.setHeight((rect.size.height * scale).round() as usize);
                let mut frame = rect_from_cg(window.frame());
                frame.size = lumora_core::Size::new(rect.size.width, rect.size.height);
                Ok((filter, config, frame, scale))
            }
        }
    }
}
