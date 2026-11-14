//! On-device text recognition with Vision (`VNRecognizeTextRequest`).
//!
//! Accurate recognition with language correction for English, Simplified
//! and Traditional Chinese and Japanese. Observations are put into reading
//! order by [`lumora_core::ocr::assemble`].

use dispatch2::{DispatchQoS, DispatchQueue, GlobalQueueIdentifier};
use lumora_core::geometry::Rect;
use lumora_core::ocr::{assemble, TextBox, RECOGNITION_LANGUAGES};
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

use crate::CaptureError;

/// Recognise the text in `image` on a background queue and call `done`
/// (on that queue) with it in reading order — empty if there is none.
pub fn recognize_text(image: CFRetained<CGImage>, done: impl FnOnce(Result<String, CaptureError>) + Send + 'static) {
    DispatchQueue::global_queue(GlobalQueueIdentifier::QualityOfService(DispatchQoS::UserInitiated))
        .exec_async(move || done(recognize_sync(&image)));
}

/// Synchronous recognition (call off the main thread; it takes a while).
pub fn recognize_sync(image: &CGImage) -> Result<String, CaptureError> {
    let (w, h) = crate::image::pixel_size(image);
    if w == 0 || h == 0 {
        return Err(CaptureError::Empty);
    }
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    let langs: Vec<Retained<NSString>> = RECOGNITION_LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
    request.setRecognitionLanguages(&NSArray::from_retained_slice(&langs));
    request.setUsesLanguageCorrection(true);
    let options = NSDictionary::new();
    // SAFETY: an empty options dictionary is valid for any key/value types.
    let handler =
        unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), image, &options) };
    let as_request: &VNRequest = &request;
    handler
        .performRequests_error(&NSArray::from_slice(&[as_request]))
        .map_err(|e| CaptureError::Framework(e.localizedDescription().to_string()))?;
    let Some(results) = request.results() else { return Ok(String::new()) };
    let (wf, hf) = (w as f64, h as f64);
    let boxes: Vec<TextBox> = results
        .iter()
        .filter_map(|obs| {
            let text = obs.topCandidates(1).firstObject()?.string().to_string();
            // SAFETY: plain property read on a live observation.
            let b = unsafe { obs.boundingBox() };
            // Vision boxes are normalised with a bottom-left origin.
            let rect = Rect::new(
                b.origin.x * wf,
                (1.0 - b.origin.y - b.size.height) * hf,
                b.size.width * wf,
                b.size.height * hf,
            );
            Some(TextBox::new(text, rect))
        })
        .collect();
    Ok(assemble(&boxes))
}
