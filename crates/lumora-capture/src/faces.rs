//! On-device face detection with Vision (`VNDetectFaceRectanglesRequest`),
//! for smart redaction.

use lumora_core::geometry::Rect;
use objc2::AnyThread;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{VNDetectFaceRectanglesRequest, VNImageRequestHandler, VNRequest};

use crate::CaptureError;

/// Face boxes in `image`, in pixels (y down). Call off the main thread.
pub fn detect_faces_sync(image: &CGImage) -> Result<Vec<Rect>, CaptureError> {
    let (w, h) = crate::image::pixel_size(image);
    if w == 0 || h == 0 {
        return Err(CaptureError::Empty);
    }
    // SAFETY: plain allocation of a Vision request.
    let request = unsafe { VNDetectFaceRectanglesRequest::new() };
    let options = NSDictionary::new();
    // SAFETY: an empty options dictionary is valid for any key/value types.
    let handler =
        unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), image, &options) };
    let as_request: &VNRequest = &request;
    handler
        .performRequests_error(&NSArray::from_slice(&[as_request]))
        .map_err(|e| CaptureError::Framework(e.localizedDescription().to_string()))?;
    // SAFETY: read after the request has finished on this thread.
    let Some(results) = (unsafe { request.results() }) else { return Ok(Vec::new()) };
    let (wf, hf) = (w as f64, h as f64);
    Ok(results
        .iter()
        .map(|obs| {
            // SAFETY: plain property read on a live observation.
            let b = unsafe { obs.boundingBox() };
            // Normalised, bottom-left origin → pixels, top-left origin.
            Rect::new(b.origin.x * wf, (1.0 - b.origin.y - b.size.height) * hf, b.size.width * wf, b.size.height * hf)
        })
        .collect())
}
