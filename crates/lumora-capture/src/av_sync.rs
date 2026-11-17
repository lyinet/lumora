//! Blocking wrappers over AVFoundation's completion-handler loaders, for
//! worker threads.
//!
//! The synchronous getters (`copyCGImageAtTime:actualTime:error:`,
//! `tracksWithMediaType:`) are deprecated and may block on I/O inside
//! AVFoundation in ways it no longer optimises for. Their replacements
//! (macOS 12 / 13, so always available on our macOS 14 minimum) call a
//! block on an AVFoundation queue — no run loop needed — so a worker thread
//! can start the request and wait on a condition variable. Never call these
//! from the main thread.

use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_av_foundation::{AVAsset, AVAssetImageGenerator, AVAssetTrack, AVMediaType};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;
use objc2_core_media::CMTime;
use objc2_foundation::{NSArray, NSError};

use crate::{CaptureError, SendBox};

/// How long a single load may take before we give up.
const TIMEOUT: Duration = Duration::from_secs(30);

type Slot<T> = Arc<(Mutex<Option<T>>, Condvar)>;

fn put<T>(slot: &Slot<T>, value: T) {
    let (lock, cv) = &**slot;
    if let Ok(mut v) = lock.lock() {
        *v = Some(value);
    }
    cv.notify_all();
}

fn wait<T>(slot: &Slot<T>, what: &str) -> Result<T, CaptureError> {
    let (lock, cv) = &**slot;
    let poisoned = || CaptureError::Framework(format!("{what}: lock poisoned"));
    let deadline = Instant::now() + TIMEOUT;
    let mut v = lock.lock().map_err(|_| poisoned())?;
    loop {
        if let Some(value) = v.take() {
            return Ok(value);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(CaptureError::Framework(format!("{what} timed out")));
        }
        v = cv.wait_timeout(v, deadline - now).map_err(|_| poisoned())?.0;
    }
}

fn describe(error: *mut NSError, fallback: &str) -> String {
    // SAFETY: AVFoundation passes nil or a valid NSError for the call.
    unsafe { error.as_ref() }.map_or_else(|| fallback.to_string(), |e| e.localizedDescription().to_string())
}

/// The frame of `generator`'s asset at `time`
/// (`generateCGImageAsynchronouslyForTime:completionHandler:`). Blocks.
pub fn image_at(generator: &AVAssetImageGenerator, time: CMTime) -> Result<CFRetained<CGImage>, CaptureError> {
    let slot: Slot<Result<CFRetained<CGImage>, String>> = Arc::new((Mutex::new(None), Condvar::new()));
    let tx = slot.clone();
    let block = RcBlock::new(move |image: *mut CGImage, _actual: CMTime, error: *mut NSError| {
        let result = match NonNull::new(image) {
            // SAFETY: the image is valid for the duration of the handler and
            // not retained for us, so take our own reference.
            Some(image) => Ok(unsafe { CFRetained::retain(image) }),
            None => Err(describe(error, "no image")),
        };
        put(&tx, result);
    });
    // SAFETY: the block only touches thread-safe state (a CGImage is
    // immutable and thread-safe; the slot is a Mutex).
    unsafe { generator.generateCGImageAsynchronouslyForTime_completionHandler(time, &block) };
    wait(&slot, "frame generation")?.map_err(CaptureError::Framework)
}

/// `asset`'s tracks of `media_type` (`loadTracksWithMediaType:completionHandler:`).
/// Blocks.
pub fn tracks(asset: &AVAsset, media_type: &AVMediaType) -> Result<Vec<Retained<AVAssetTrack>>, CaptureError> {
    type Tracks = SendBox<Vec<Retained<AVAssetTrack>>>;
    let slot: Slot<Result<Tracks, String>> = Arc::new((Mutex::new(None), Condvar::new()));
    let tx = slot.clone();
    let block = RcBlock::new(move |tracks: *mut NSArray<AVAssetTrack>, error: *mut NSError| {
        // SAFETY: AVFoundation passes nil or a valid array for the call.
        let result = match unsafe { tracks.as_ref() } {
            // SAFETY: asset tracks are immutable snapshots of a file asset;
            // AVFoundation documents them as safe to use from any thread.
            Some(tracks) => Ok(unsafe { SendBox::new(tracks.iter().collect::<Vec<_>>()) }),
            None => Err(describe(error, "no tracks")),
        };
        put(&tx, result);
    });
    // SAFETY: the block only touches thread-safe state (see above).
    unsafe { asset.loadTracksWithMediaType_completionHandler(media_type, &block) };
    Ok(wait(&slot, "loading tracks")?.map_err(CaptureError::Framework)?.into_inner())
}
