//! Screen recording: `SCStream` → `AVAssetWriter` (MP4, H.264 or HEVC).
//!
//! ```text
//!  SCStream ──(sample buffers on `com.lumora.recorder` serial queue)──▶ StreamOutput
//!                                                                         │
//!                                     first complete frame starts session │
//!                                                                         ▼
//!                                         AVAssetWriter ◀── video / system audio / mic inputs
//! ```
//!
//! All writer access happens on the single serial sample queue (or, during
//! stop, strictly after the stream has stopped delivering), which is what
//! makes the `SendBox` wrappers sound.
//!
//! Our own windows (recording border, stop control, HUDs) are excluded with
//! `SCContentFilter(display:excludingWindows:)`, so they are visible to the
//! user but never appear in the video — except the window numbers listed in
//! [`RecordOptions::allow_windows`] (the click-ripple overlays), which must.
//!
//! Microphone: on macOS 15+ ScreenCaptureKit delivers it; on macOS 14 an
//! `AVCaptureSession` ([`crate::mic`]) delivers it on the same serial queue,
//! retimed from its synchronisation clock to the host clock SC uses.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use lumora_core::geometry::Rect;
use lumora_core::settings::{Codec, Recording};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus, AVFileTypeMPEG4, AVMediaTypeAudio, AVMediaTypeVideo,
    AVVideoAverageBitRateKey, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoCodecTypeHEVC,
    AVVideoCompressionPropertiesKey, AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey,
    AVVideoMaxKeyFrameIntervalKey, AVVideoProfileLevelH264HighAutoLevel, AVVideoProfileLevelKey, AVVideoWidthKey,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCFrameStatus, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType,
};

use crate::content::{ns_error, rect_to_cg, ShareableContent};
use crate::{CaptureError, SendBox};

/// `kCVPixelFormatType_32BGRA` ('BGRA').
const PIXEL_FORMAT_BGRA: u32 = 0x4247_5241;
/// `kAudioFormatMPEG4AAC` ('aac ').
const AUDIO_FORMAT_AAC: u32 = 0x6161_6320;

/// What and how to record.
#[derive(Debug, Clone)]
pub struct RecordOptions {
    /// Display to record (`CGDirectDisplayID`).
    pub display_id: u32,
    /// Sub-rectangle in global top-left points; `None` records the display.
    pub area: Option<Rect>,
    pub settings: Recording,
    /// Destination `.mp4`; must not exist yet.
    pub output: PathBuf,
    /// Lumora window numbers that must appear in the video (click ripples).
    pub allow_windows: Vec<u32>,
}

/// Asynchronous notifications from a running recorder (arbitrary queue).
#[derive(Debug, Clone, PartialEq)]
pub enum RecorderEvent {
    /// The first frame was written; the clock is running. `host_time` is
    /// that frame's presentation time on the host clock (seconds), the
    /// zero point of the movie's timeline.
    Started { host_time: f64 },
    /// The microphone could not be started (permission denied, device
    /// gone…); recording continues without it.
    MicUnavailable(CaptureError),
    /// The stream stopped unexpectedly (display unplugged, permission
    /// revoked…). The partial file is finalised if possible.
    Failed(CaptureError),
}

type EventSink = Box<dyn Fn(RecorderEvent) + Send + Sync>;

struct WriterState {
    writer: SendBox<Retained<AVAssetWriter>>,
    video: SendBox<Retained<AVAssetWriterInput>>,
    audio: Option<SendBox<Retained<AVAssetWriterInput>>>,
    mic: Option<SendBox<Retained<AVAssetWriterInput>>>,
    session_started: bool,
    /// Host-clock seconds of the first video frame.
    session_start: Option<f64>,
    /// Set once stop begins; no further appends are allowed.
    closed: bool,
}

struct Shared {
    state: Mutex<WriterState>,
    on_event: EventSink,
    failed: AtomicBool,
}

pub struct OutputIvars {
    shared: Arc<Shared>,
}

define_class!(
    /// Receives sample buffers and stream lifecycle callbacks.
    #[unsafe(super(NSObject))]
    #[name = "LumoraStreamOutput"]
    #[ivars = OutputIvars]
    struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            self.ivars().shared.handle_sample(sample, kind);
        }
    }

    unsafe impl SCStreamDelegate for StreamOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop(&self, _stream: &SCStream, error: &NSError) {
            let shared = &self.ivars().shared;
            if !shared.failed.swap(true, Ordering::SeqCst) {
                let err = ns_error(error as *const NSError as *mut NSError);
                (shared.on_event)(RecorderEvent::Failed(err));
            }
        }
    }
);

impl StreamOutput {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars { shared });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Whether a screen sample carries a complete, new frame. Idle/blank frames
/// (nothing changed on screen) have no pixels and must not be appended.
fn is_complete_frame(sample: &CMSampleBuffer) -> bool {
    // SAFETY: read-only CoreMedia accessors on a valid buffer.
    unsafe {
        if !sample.is_valid() || sample.image_buffer().is_none() {
            return false;
        }
        let Some(attachments) = sample.sample_attachments_array(false) else { return false };
        // SAFETY: CFArray is toll-free bridged to NSArray, and SC's sample
        // attachments are dictionaries keyed by SCStreamFrameInfo strings.
        let array: &NSArray<NSDictionary<NSString, AnyObject>> = &*(&*attachments
            as *const objc2_core_foundation::CFArray)
            .cast::<NSArray<NSDictionary<NSString, AnyObject>>>();
        let Some(info) = array.firstObject() else { return false };
        let Some(status) = info.objectForKey(SCStreamFrameInfoStatus) else { return false };
        let Ok(number) = status.downcast::<NSNumber>() else { return false };
        SCFrameStatus(number.integerValue()) == SCFrameStatus::Complete
    }
}

impl Shared {
    fn handle_sample(&self, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
        let Ok(mut st) = self.state.lock() else { return };
        if st.closed {
            return;
        }
        // SAFETY: all writer calls happen on the serial sample queue while
        // holding the lock; see module docs.
        unsafe {
            if kind == SCStreamOutputType::Screen {
                if !is_complete_frame(sample) {
                    return;
                }
                if !st.session_started {
                    let pts = sample.presentation_time_stamp();
                    st.writer.startSessionAtSourceTime(pts);
                    st.session_started = true;
                    st.session_start = Some(pts.seconds());
                    (self.on_event)(RecorderEvent::Started { host_time: pts.seconds() });
                }
                if st.video.isReadyForMoreMediaData() && !st.video.appendSampleBuffer(sample) {
                    log::warn!("video append failed: {:?}", st.writer.error().map(|e| e.localizedDescription()));
                }
            } else if st.session_started && sample.is_valid() {
                let input = if kind == SCStreamOutputType::Audio { st.audio.as_ref() } else { st.mic.as_ref() };
                if let Some(input) = input {
                    if input.isReadyForMoreMediaData() {
                        input.appendSampleBuffer(sample);
                    }
                }
            }
        }
    }
}

impl Shared {
    /// macOS 14 microphone path: retime onto the host clock and append.
    fn handle_mic(&self, sample: &CMSampleBuffer, clock: &crate::mic::HostClock) {
        let Ok(st) = self.state.lock() else { return };
        if st.closed || !st.session_started {
            return;
        }
        let Some(input) = st.mic.as_ref() else { return };
        // SAFETY: on the serial sample queue while holding the lock.
        unsafe {
            if !sample.is_valid() {
                return;
            }
            let Some((pts, dur, buffer)) = clock.to_host(sample) else { return };
            if !lumora_core::click_log::time::audio_overlaps_session(pts, dur, st.session_start) {
                return;
            }
            if input.isReadyForMoreMediaData() {
                input.appendSampleBuffer(&buffer);
            }
        }
    }
}

/// A running recording. Dropping it without calling [`Recorder::stop`]
/// stops the stream and discards the file.
pub struct Recorder {
    stream: SendBox<Retained<SCStream>>,
    /// Retained until the stream has fully stopped (taken in `stop`).
    output: Option<SendBox<Retained<StreamOutput>>>,
    queue: dispatch2::DispatchRetained<DispatchQueue>,
    shared: Arc<Shared>,
    path: PathBuf,
    stopped: bool,
    /// macOS 14 microphone capture (stopped before the stream).
    mic: Option<crate::mic::MicSession>,
    capture_frame: Rect,
    video_size: (u32, u32),
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder").field("path", &self.path).finish_non_exhaustive()
    }
}

fn constant<T: ?Sized>(c: Option<&'static T>, name: &str) -> Result<&'static T, CaptureError> {
    c.ok_or_else(|| CaptureError::Framework(format!("AVFoundation constant {name} unavailable")))
}

/// Round down to an even number ≥ 2 (H.264/HEVC require even dimensions).
fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

fn build_writer(path: &Path, width: u32, height: u32, s: &Recording) -> Result<WriterState, CaptureError> {
    let url = crate::clipboard::file_url(path)
        .ok_or(CaptureError::Write { path: path.display().to_string(), reason: "invalid path".into() })?;
    // SAFETY: constructing AVFoundation objects with valid arguments.
    unsafe {
        let file_type = constant(AVFileTypeMPEG4, "AVFileTypeMPEG4")?;
        let writer = AVAssetWriter::initWithURL_fileType_error(AVAssetWriter::alloc(), &url, file_type)
            .map_err(|e| CaptureError::Framework(e.localizedDescription().to_string()))?;

        let codec = match s.codec {
            Codec::H264 => constant(AVVideoCodecTypeH264, "AVVideoCodecTypeH264")?,
            Codec::Hevc => constant(AVVideoCodecTypeHEVC, "AVVideoCodecTypeHEVC")?,
        };
        let bitrate = NSNumber::numberWithLongLong(s.bitrate_for(width, height) as i64);
        let fps = NSNumber::numberWithUnsignedInt(s.fps);
        let keyframe = NSNumber::numberWithUnsignedInt(s.fps * 2);
        let mut ckeys: Vec<&NSString> = vec![
            constant(AVVideoAverageBitRateKey, "AVVideoAverageBitRateKey")?,
            constant(AVVideoExpectedSourceFrameRateKey, "AVVideoExpectedSourceFrameRateKey")?,
            constant(AVVideoMaxKeyFrameIntervalKey, "AVVideoMaxKeyFrameIntervalKey")?,
        ];
        let mut cvals: Vec<&AnyObject> = vec![&bitrate, &fps, &keyframe];
        if s.codec == Codec::H264 {
            ckeys.push(constant(AVVideoProfileLevelKey, "AVVideoProfileLevelKey")?);
            cvals.push(constant(AVVideoProfileLevelH264HighAutoLevel, "AVVideoProfileLevelH264HighAutoLevel")?);
        }
        let compression = NSDictionary::<NSString, AnyObject>::from_slices(&ckeys, &cvals);
        let w = NSNumber::numberWithUnsignedInt(width);
        let h = NSNumber::numberWithUnsignedInt(height);
        let video_settings = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                constant(AVVideoCodecKey, "AVVideoCodecKey")?,
                constant(AVVideoWidthKey, "AVVideoWidthKey")?,
                constant(AVVideoHeightKey, "AVVideoHeightKey")?,
                constant(AVVideoCompressionPropertiesKey, "AVVideoCompressionPropertiesKey")?,
            ],
            &[codec, &w, &h, &compression],
        );
        let video = AVAssetWriterInput::initWithMediaType_outputSettings(
            AVAssetWriterInput::alloc(),
            constant(AVMediaTypeVideo, "AVMediaTypeVideo")?,
            Some(&video_settings),
        );
        video.setExpectsMediaDataInRealTime(true);
        if !writer.canAddInput(&video) {
            return Err(CaptureError::Framework("cannot add video input".into()));
        }
        writer.addInput(&video);

        let make_audio = || -> Result<Option<Retained<AVAssetWriterInput>>, CaptureError> {
            // Audio settings keys are plain strings equal to their names
            // (AVFAudio's AVFormatIDKey etc.).
            let fmt = NSNumber::numberWithUnsignedInt(AUDIO_FORMAT_AAC);
            let rate = NSNumber::numberWithDouble(48_000.0);
            let channels = NSNumber::numberWithUnsignedInt(2);
            let bitrate = NSNumber::numberWithUnsignedInt(128_000);
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    &*NSString::from_str("AVFormatIDKey"),
                    &*NSString::from_str("AVSampleRateKey"),
                    &*NSString::from_str("AVNumberOfChannelsKey"),
                    &*NSString::from_str("AVEncoderBitRateKey"),
                ],
                &[&*fmt, &*rate, &*channels, &*bitrate],
            );
            let input = AVAssetWriterInput::initWithMediaType_outputSettings(
                AVAssetWriterInput::alloc(),
                constant(AVMediaTypeAudio, "AVMediaTypeAudio")?,
                Some(&settings),
            );
            input.setExpectsMediaDataInRealTime(true);
            if writer.canAddInput(&input) {
                writer.addInput(&input);
                Ok(Some(input))
            } else {
                Ok(None)
            }
        };
        let audio = if s.system_audio { make_audio()? } else { None };
        let mic = if s.microphone { make_audio()? } else { None };

        if !writer.startWriting() {
            let reason = writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
            return Err(CaptureError::Framework(format!("could not start writing: {reason}")));
        }

        // SAFETY (SendBox): only touched on the serial sample queue / after
        // the stream stops; see module docs.
        Ok(WriterState {
            writer: SendBox::new(writer),
            video: SendBox::new(video),
            audio: audio.map(|a| SendBox::new(a)),
            mic: mic.map(|m| SendBox::new(m)),
            session_started: false,
            session_start: None,
            closed: false,
        })
    }
}

impl Recorder {
    /// Start recording. Returns once the stream has been asked to start;
    /// [`RecorderEvent::Started`] follows when the first frame lands.
    ///
    /// `content` should be fetched *after* Lumora's own recording chrome
    /// (border, stop button) is on screen so those windows get excluded.
    pub fn start(
        content: &ShareableContent,
        opts: RecordOptions,
        on_event: impl Fn(RecorderEvent) + Send + Sync + 'static,
    ) -> Result<Recorder, CaptureError> {
        let display_info = content
            .displays()
            .into_iter()
            .find(|d| d.id == opts.display_id)
            .ok_or(CaptureError::TargetGone("display"))?;
        let display = content.sc_display(opts.display_id).ok_or(CaptureError::TargetGone("display"))?;
        let scale = display_info.scale;
        let frame = match opts.area {
            Some(a) => a.intersection(&display_info.frame).ok_or(CaptureError::Empty)?.integral(scale),
            None => display_info.frame,
        };
        let (pw, ph) = frame.pixel_size(scale);
        let (width, height) = (even(pw), even(ph));
        let s = &opts.settings;

        let state = build_writer(&opts.output, width, height, s)?;
        let shared =
            Arc::new(Shared { state: Mutex::new(state), on_event: Box::new(on_event), failed: AtomicBool::new(false) });
        let output = StreamOutput::new(shared.clone());
        let queue = DispatchQueue::new("com.lumora.recorder", None);

        // SAFETY: configuring fresh objects, then handing them to SCStream.
        let stream = unsafe {
            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &content.own_windows_except(&opts.allow_windows),
            );
            let config = SCStreamConfiguration::new();
            config.setWidth(width as usize);
            config.setHeight(height as usize);
            if opts.area.is_some() {
                config.setSourceRect(rect_to_cg(frame.relative_to(&display_info.frame)));
            }
            config.setMinimumFrameInterval(CMTime::new(1, s.fps as i32));
            config.setPixelFormat(PIXEL_FORMAT_BGRA);
            config.setShowsCursor(s.show_cursor);
            config.setQueueDepth(6);
            config.setScalesToFit(false);
            if s.system_audio {
                config.setCapturesAudio(true);
                config.setSampleRate(48_000);
                config.setChannelCount(2);
                config.setExcludesCurrentProcessAudio(true);
            }
            // Microphone capture through ScreenCaptureKit is macOS 15+; on
            // 14 an AVCaptureSession takes over (started below).
            let mic_supported = config.respondsToSelector(sel!(setCaptureMicrophone:));
            if s.microphone && mic_supported {
                config.setCaptureMicrophone(true);
                if let Some(id) = &s.microphone_device {
                    if config.respondsToSelector(sel!(setMicrophoneCaptureDeviceID:)) {
                        config.setMicrophoneCaptureDeviceID(Some(&NSString::from_str(id)));
                    }
                }
            }

            let delegate = ProtocolObject::from_ref(&*output);
            let stream =
                SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, Some(delegate));
            let out = ProtocolObject::from_ref(&*output);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(out, SCStreamOutputType::Screen, Some(&queue))
                .map_err(|e| CaptureError::Framework(e.localizedDescription().to_string()))?;
            if s.system_audio {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(out, SCStreamOutputType::Audio, Some(&queue))
                    .map_err(|e| CaptureError::Framework(e.localizedDescription().to_string()))?;
            }
            if s.microphone && mic_supported {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(out, SCStreamOutputType::Microphone, Some(&queue))
                    .map_err(|e| CaptureError::Framework(e.localizedDescription().to_string()))?;
            }
            stream
        };

        // macOS 14: microphone through AVCaptureSession on the same queue.
        let mic = if s.microphone && !crate::mic::sc_microphone_supported() {
            let shared_mic = shared.clone();
            match crate::mic::MicSession::start(s.microphone_device.as_deref(), &queue, move |sample, clock| {
                shared_mic.handle_mic(sample, clock)
            }) {
                Ok(m) => Some(m),
                Err(e) => {
                    log::warn!("microphone unavailable: {e}");
                    (shared.on_event)(RecorderEvent::MicUnavailable(e));
                    None
                }
            }
        } else {
            None
        };

        let shared_for_start = shared.clone();
        let started = RcBlock::new(move |error: *mut NSError| {
            if !error.is_null() && !shared_for_start.failed.swap(true, Ordering::SeqCst) {
                (shared_for_start.on_event)(RecorderEvent::Failed(ns_error(error)));
            }
        });
        // SAFETY: valid stream and block.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };

        // SAFETY (SendBox): SCStream is internally synchronised; we only call
        // start/stop on it. The output object is only retained, never used.
        Ok(Recorder {
            stream: unsafe { SendBox::new(stream) },
            output: Some(unsafe { SendBox::new(output) }),
            queue,
            shared,
            path: opts.output,
            stopped: false,
            mic,
            capture_frame: frame,
            video_size: (width, height),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Recorded area in global Quartz points (after clipping/integral).
    pub fn capture_frame(&self) -> Rect {
        self.capture_frame
    }

    /// Encoded video size in pixels.
    pub fn video_size(&self) -> (u32, u32) {
        self.video_size
    }

    /// Stop and finalise the file. `completion` runs on an arbitrary queue
    /// with the finished file path.
    pub fn stop(mut self, completion: impl FnOnce(Result<PathBuf, CaptureError>) + Send + 'static) {
        self.stopped = true;
        if let Some(mic) = self.mic.take() {
            mic.stop();
        }
        let shared = self.shared.clone();
        let path = self.path.clone();
        let queue = self.queue.clone();
        // Keep the output object alive until the stream has fully stopped.
        let keep_alive = Mutex::new(Some((self.output.take(), completion)));
        let block = RcBlock::new(move |_error: *mut NSError| {
            let Some((output, completion)) = keep_alive.lock().ok().and_then(|mut k| k.take()) else { return };
            let shared = shared.clone();
            let path = path.clone();
            // Hop onto the sample queue so finalisation is ordered after any
            // in-flight sample callbacks.
            queue.exec_async(move || {
                let _output = output;
                finish(&shared, path, completion);
            });
        });
        // SAFETY: valid stream and block.
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };
    }
}

/// Mark inputs finished and finalise the MP4.
fn finish(shared: &Shared, path: PathBuf, completion: impl FnOnce(Result<PathBuf, CaptureError>) + Send + 'static) {
    let Ok(mut st) = shared.state.lock() else {
        return completion(Err(CaptureError::Framework("recorder state poisoned".into())));
    };
    st.closed = true;
    if !st.session_started {
        // SAFETY: cancelling an unused writer.
        unsafe { st.writer.cancelWriting() };
        let _ = std::fs::remove_file(&path);
        return completion(Err(CaptureError::Framework("no frames were recorded".into())));
    }
    // SAFETY: we're on the sample queue and the stream has stopped, so no
    // concurrent appends can happen.
    unsafe {
        st.video.markAsFinished();
        if let Some(a) = &st.audio {
            a.markAsFinished();
        }
        if let Some(m) = &st.mic {
            m.markAsFinished();
        }
        // SAFETY (SendBox): the writer is only used again inside its own
        // completion handler, after all other access has ended.
        let writer = SendBox::new(st.writer.clone());
        let slot = Mutex::new(Some(completion));
        let done = RcBlock::new(move || {
            let Some(cb) = slot.lock().ok().and_then(|mut s| s.take()) else { return };
            if writer.status() == AVAssetWriterStatus::Completed {
                cb(Ok(path.clone()));
            } else {
                let reason = writer.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
                cb(Err(CaptureError::Write { path: path.display().to_string(), reason }));
            }
        });
        st.writer.finishWritingWithCompletionHandler(&done);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if !self.stopped {
            if let Ok(mut st) = self.shared.state.lock() {
                st.closed = true;
                // SAFETY: abandoning the writer; the file is discarded.
                unsafe { st.writer.cancelWriting() };
            }
            // SAFETY: stopping a running stream with no completion handler.
            unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        }
    }
}

/// Poster frame for a finished recording (first frame, at most
/// `max_side` pixels on its longer side) for the Quick Access card.
pub fn thumbnail(
    path: &Path,
    max_side: f64,
) -> Option<objc2_core_foundation::CFRetained<objc2_core_graphics::CGImage>> {
    use objc2_av_foundation::{AVAsset, AVAssetImageGenerator};
    let url = crate::clipboard::file_url(path)?;
    // SAFETY: synchronous image generation from a local file; all
    // arguments are valid. `actualTime` may be NULL.
    unsafe {
        let asset = AVAsset::assetWithURL(&url);
        let generator = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
        generator.setAppliesPreferredTrackTransform(true);
        generator.setMaximumSize(objc2_core_foundation::CGSize::new(max_side, max_side));
        #[allow(deprecated)] // async replacement is macOS 13+, but sync is simpler here and off the main thread.
        let image = generator.copyCGImageAtTime_actualTime_error(CMTime::new(0, 600), std::ptr::null_mut()).ok()?;
        // Transfer the +1 reference from objc2's `Retained` to `CFRetained`.
        let raw = std::ptr::NonNull::new(Retained::into_raw(image))?;
        Some(objc2_core_foundation::CFRetained::from_raw(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::even;

    #[test]
    fn even_dimensions() {
        assert_eq!(even(1921), 1920);
        assert_eq!(even(1080), 1080);
        assert_eq!(even(1), 2);
    }
}
