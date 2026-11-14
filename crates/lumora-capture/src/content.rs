//! Enumerating capturable displays and windows.

use std::sync::Mutex;

use block2::RcBlock;
use lumora_core::geometry::Rect;
use lumora_core::snap::WindowInfo;
use objc2::rc::Retained;
use objc2_core_foundation::{CFArray, CGRect};
use objc2_core_graphics::{CGDisplayCopyDisplayMode, CGDisplayMode, CGWindowListCreate, CGWindowListOption};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{SCDisplay, SCShareableContent, SCWindow};

use crate::CaptureError;

/// A display, in global top-left points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayInfo {
    /// `CGDirectDisplayID`.
    pub id: u32,
    pub frame: Rect,
    /// Backing scale factor (2.0 on Retina).
    pub scale: f64,
}

/// An immutable snapshot of what can be captured right now.
pub struct ShareableContent {
    inner: Retained<SCShareableContent>,
}

// SAFETY: `SCShareableContent` is an immutable snapshot object; Apple's
// Swift overlay marks it `Sendable`.
unsafe impl Send for ShareableContent {}
unsafe impl Sync for ShareableContent {}

impl std::fmt::Debug for ShareableContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareableContent").field("displays", &self.displays()).finish_non_exhaustive()
    }
}

pub(crate) fn rect_from_cg(r: CGRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

pub(crate) fn rect_to_cg(r: Rect) -> CGRect {
    CGRect::new(
        objc2_core_foundation::CGPoint::new(r.min_x(), r.min_y()),
        objc2_core_foundation::CGSize::new(r.width(), r.height()),
    )
}

/// Convert a (possibly null) `NSError*` into our error type.
pub(crate) fn ns_error(error: *mut NSError) -> CaptureError {
    // SAFETY: the pointer comes straight from a framework completion handler
    // and is either null or a valid NSError for the duration of the call.
    match unsafe { error.as_ref() } {
        Some(e) => {
            // SCStreamErrorUserDeclined = -3801.
            if e.code() == -3801 {
                CaptureError::PermissionDenied
            } else {
                CaptureError::Framework(e.localizedDescription().to_string())
            }
        }
        None => CaptureError::Framework("unknown error".into()),
    }
}

/// Backing scale of a display, from its current mode (pixel width / point
/// width). Falls back to 2.0, the common case on modern Macs.
pub fn display_scale(display_id: u32) -> f64 {
    let mode = CGDisplayCopyDisplayMode(display_id);
    let points = CGDisplayMode::width(mode.as_deref());
    let pixels = CGDisplayMode::pixel_width(mode.as_deref());
    if points > 0 && pixels > 0 {
        pixels as f64 / points as f64
    } else {
        2.0
    }
}

/// Window IDs currently on screen, front to back (`CGWindowListCreate`).
fn window_z_order() -> Vec<u32> {
    let opts = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCreate(opts, 0) else { return Vec::new() };
    let list: &CFArray = &list;
    (0..list.count())
        // SAFETY: CGWindowListCreate returns an array whose "values" are
        // CGWindowIDs stored directly in the pointer slots (no callbacks).
        .map(|i| unsafe { list.value_at_index(i) } as usize as u32)
        .collect()
}

impl ShareableContent {
    /// Fetch on-screen content asynchronously. `completion` runs on an
    /// arbitrary ScreenCaptureKit queue.
    pub fn fetch(completion: impl FnOnce(Result<ShareableContent, CaptureError>) + Send + 'static) {
        // Blocks are `Fn`; stash the `FnOnce` and take it on first call.
        let slot = Mutex::new(Some(completion));
        let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
            let Some(cb) = slot.lock().ok().and_then(|mut s| s.take()) else { return };
            // SAFETY: `content` is null or a valid object for this call; we
            // retain it to extend its lifetime.
            let result = match unsafe { Retained::retain(content) } {
                Some(inner) => Ok(ShareableContent { inner }),
                None => Err(ns_error(error)),
            };
            cb(result);
        });
        // SAFETY: valid block with the expected signature.
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                true, true, &block,
            );
        }
    }

    /// All displays with their global frames and scale.
    pub fn displays(&self) -> Vec<DisplayInfo> {
        // SAFETY: plain property reads on an immutable snapshot.
        unsafe { self.inner.displays() }
            .iter()
            .map(|d| {
                let id = unsafe { d.displayID() };
                DisplayInfo { id, frame: rect_from_cg(unsafe { d.frame() }), scale: display_scale(id) }
            })
            .collect()
    }

    /// Capturable windows, **front to back**, excluding our own process.
    pub fn windows(&self) -> Vec<WindowInfo> {
        let own_pid = std::process::id() as i32;
        let order = window_z_order();
        let rank = |id: u32| order.iter().position(|&w| w == id).unwrap_or(usize::MAX);
        let mut out: Vec<(usize, WindowInfo)> = unsafe { self.inner.windows() }
            .iter()
            .filter_map(|w| unsafe {
                let app = w.owningApplication();
                if app.as_ref().map(|a| a.processID()) == Some(own_pid) || !w.isOnScreen() {
                    return None;
                }
                let id = w.windowID();
                Some((
                    rank(id),
                    WindowInfo {
                        id,
                        frame: rect_from_cg(w.frame()),
                        app_name: app.map(|a| a.applicationName().to_string()).unwrap_or_default(),
                        title: w.title().map(|t| t.to_string()).unwrap_or_default(),
                        layer: w.windowLayer() as i32,
                    },
                ))
            })
            .collect();
        out.sort_by_key(|(r, _)| *r);
        out.into_iter().map(|(_, w)| w).collect()
    }

    pub(crate) fn sc_display(&self, id: u32) -> Option<Retained<SCDisplay>> {
        unsafe { self.inner.displays() }.iter().find(|d| unsafe { d.displayID() } == id)
    }

    pub(crate) fn sc_window(&self, id: u32) -> Option<Retained<SCWindow>> {
        unsafe { self.inner.windows() }.iter().find(|w| unsafe { w.windowID() } == id)
    }

    /// Our own windows (overlay, HUDs, recording border) so recordings can
    /// exclude them via `SCContentFilter`.
    pub(crate) fn own_windows(&self) -> Retained<NSArray<SCWindow>> {
        let pid = std::process::id() as i32;
        let mine: Vec<Retained<SCWindow>> = unsafe { self.inner.windows() }
            .iter()
            .filter(|w| unsafe { w.owningApplication() }.is_some_and(|a| unsafe { a.processID() } == pid))
            .collect();
        NSArray::from_retained_slice(&mine)
    }

    /// The display whose frame contains the centre of `rect` (or the one it
    /// overlaps most).
    pub fn display_for_rect(&self, rect: Rect) -> Option<DisplayInfo> {
        let displays = self.displays();
        displays
            .iter()
            .find(|d| d.frame.contains(rect.mid()))
            .or_else(|| {
                displays.iter().max_by(|a, b| {
                    let ia = a.frame.intersection(&rect).map_or(0.0, |r| r.area());
                    let ib = b.frame.intersection(&rect).map_or(0.0, |r| r.area());
                    ia.total_cmp(&ib)
                })
            })
            .copied()
    }
}
