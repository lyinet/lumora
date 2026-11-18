//! Enumerating capturable displays and windows.

use std::sync::Mutex;

use block2::RcBlock;
use lumora_core::geometry::Rect;
use lumora_core::snap::WindowInfo;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_foundation::{CFArray, CGRect};
use objc2_core_graphics::{CGDisplayCopyDisplayMode, CGDisplayMode, CGWindowListCreate, CGWindowListOption};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{SCContentFilter, SCDisplay, SCRunningApplication, SCShareableContent, SCWindow};

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
            let (domain, code) = (e.domain().to_string(), e.code());
            log::warn!("framework error {domain} {code}: {}", e.localizedDescription());
            // SCStreamErrorUserDeclined: TCC refused (missing or stale grant).
            if code == -3801 {
                CaptureError::PermissionDenied
            } else {
                CaptureError::Framework(e.localizedDescription().to_string())
            }
        }
        None => {
            log::warn!("framework call failed without an NSError");
            CaptureError::Framework("unknown error".into())
        }
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
    /// Fetch capturable content asynchronously. `completion` runs on an
    /// arbitrary ScreenCaptureKit queue.
    ///
    /// Off-screen windows are included too (so Lumora's own panels that are
    /// created but not yet ordered in, and Lumora itself as an application,
    /// are in the snapshot and can be excluded); [`Self::windows`] still
    /// lists on-screen windows only.
    pub fn fetch(completion: impl FnOnce(Result<ShareableContent, CaptureError>) + Send + 'static) {
        // Blocks are `Fn`; stash the `FnOnce` and take it on first call.
        let slot = Mutex::new(Some(completion));
        let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
            let Some(cb) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
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
                true, false, &block,
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

    /// Lumora's windows minus `allow` (window numbers that must stay in the
    /// recording, e.g. the click-ripple overlays).
    pub(crate) fn own_windows_except(&self, allow: &[u32]) -> Retained<NSArray<SCWindow>> {
        let pid = std::process::id() as i32;
        let mine: Vec<Retained<SCWindow>> = unsafe { self.inner.windows() }
            .iter()
            .filter(|w| unsafe { w.owningApplication() }.is_some_and(|a| unsafe { a.processID() } == pid))
            .filter(|w| !allow.contains(&unsafe { w.windowID() }))
            .collect();
        NSArray::from_retained_slice(&mine)
    }

    /// Lumora itself as ScreenCaptureKit lists it (`None` when the snapshot
    /// doesn't list it).
    fn own_application(&self) -> Option<Retained<SCRunningApplication>> {
        let pid = std::process::id() as i32;
        // SAFETY: plain property reads on an immutable snapshot.
        unsafe { self.inner.applications() }.iter().find(|a| unsafe { a.processID() } == pid)
    }

    /// Lumora is listed as an application, so captures can exclude all of
    /// its windows (see [`Self::display_filter`]).
    pub fn excludes_own_app(&self) -> bool {
        self.own_application().is_some()
    }

    /// Window numbers of Lumora's windows in this snapshot (for logs).
    pub fn own_window_ids(&self) -> Vec<u32> {
        let pid = std::process::id() as i32;
        unsafe { self.inner.windows() }
            .iter()
            .filter(|w| unsafe { w.owningApplication() }.is_some_and(|a| unsafe { a.processID() } == pid))
            .map(|w| unsafe { w.windowID() })
            .collect()
    }

    /// A filter for `display` that never shows Lumora's own windows (the
    /// overlay, HUDs, toasts, the recording border, the scrolling control),
    /// except the window numbers in `allow` (the click ripples a recording
    /// must show).
    ///
    /// Excludes Lumora as an *application*, so windows created or ordered in
    /// after this snapshot are excluded too. Falls back to excluding the
    /// windows the snapshot lists when Lumora isn't listed as an application.
    pub(crate) fn display_filter(&self, display: &SCDisplay, allow: &[u32]) -> Retained<SCContentFilter> {
        // SAFETY: building a filter from objects of this snapshot.
        unsafe {
            match self.own_application() {
                Some(app) => {
                    let keep: Vec<Retained<SCWindow>> = self
                        .inner
                        .windows()
                        .iter()
                        .filter(|w| {
                            allow.contains(&w.windowID())
                                && w.owningApplication().is_some_and(|a| a.processID() == app.processID())
                        })
                        .collect();
                    SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                        SCContentFilter::alloc(),
                        display,
                        &NSArray::from_retained_slice(&[app]),
                        &NSArray::from_retained_slice(&keep),
                    )
                }
                None => {
                    log::warn!("capture filter: Lumora not listed as an application; excluding its listed windows");
                    SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        display,
                        &self.own_windows_except(allow),
                    )
                }
            }
        }
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

/// The window under a point, as the window server reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowHit {
    pub pid: i32,
    pub owner_name: Option<String>,
    /// `kCGWindowName` (needs Screen Recording permission, which recording
    /// already requires). No Accessibility API involved.
    pub title: Option<String>,
}

/// The front-most visible window of another process containing `p`
/// (global Quartz points). Lumora's own windows are skipped.
pub fn window_at(p: lumora_core::geometry::Point) -> Option<WindowHit> {
    use objc2::runtime::AnyObject;
    use objc2_core_graphics::CGWindowListCopyWindowInfo;
    use objc2_foundation::{NSDictionary, NSNumber, NSString};
    let list = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )?;
    // SAFETY: CFArray of CFDictionary is toll-free bridged to NSArray of
    // NSDictionary; the list is ordered front to back.
    let array: &NSArray<NSDictionary<NSString, AnyObject>> = unsafe { &*(&*list as *const CFArray).cast() };
    let own = std::process::id() as isize;
    let num = |d: &NSDictionary<NSString, AnyObject>, k: &str| {
        d.objectForKey(&NSString::from_str(k)).and_then(|v| v.downcast::<NSNumber>().ok())
    };
    let string = |d: &NSDictionary<NSString, AnyObject>, k: &str| {
        d.objectForKey(&NSString::from_str(k))
            .and_then(|v| v.downcast::<NSString>().ok())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    };
    array.iter().find_map(|d| {
        let pid = num(&d, "kCGWindowOwnerPID")?.integerValue();
        if pid == own || num(&d, "kCGWindowAlpha").is_some_and(|a| a.doubleValue() <= 0.0) {
            return None;
        }
        let bounds = d.objectForKey(&NSString::from_str("kCGWindowBounds"))?;
        let bounds = bounds.downcast::<NSDictionary>().ok()?;
        // SAFETY: kCGWindowBounds is a dictionary of NSString → NSNumber.
        let bounds: &NSDictionary<NSString, AnyObject> = unsafe { &*(&*bounds as *const NSDictionary).cast() };
        let f = |k: &str| num(bounds, k).map(|n| n.doubleValue());
        let r = Rect::new(f("X")?, f("Y")?, f("Width")?, f("Height")?);
        r.contains(p).then(|| WindowHit {
            pid: pid as i32,
            owner_name: string(&d, "kCGWindowOwnerName"),
            title: string(&d, "kCGWindowName"),
        })
    })
}
