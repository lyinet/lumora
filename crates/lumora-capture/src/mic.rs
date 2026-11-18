//! Microphone: device list, permission, and the macOS 14 capture path.
//!
//! ScreenCaptureKit only captures the microphone on macOS 15+
//! (`SCStreamConfiguration.captureMicrophone`). On macOS 14 the recorder
//! runs an `AVCaptureSession` with the chosen audio device next to the
//! `SCStream`; its sample buffers are delivered on the recorder's serial
//! sample queue, converted from the session's synchronisation clock to the
//! host clock (which SC video uses), and appended to the writer's mic input.

use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureAudioDataOutput, AVCaptureAudioDataOutputSampleBufferDelegate, AVCaptureConnection,
    AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureDeviceTypeExternal, AVCaptureDeviceTypeMicrophone, AVCaptureOutput, AVCaptureSession, AVMediaTypeAudio,
};
use objc2_core_media::{CMClock, CMSampleBuffer, CMSampleTimingInfo, CMSyncConvertTime, CMTime};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSObject, NSString};
use objc2_screen_capture_kit::SCStreamConfiguration;

use crate::{CaptureError, SendBox};

/// `kAudioFormatLinearPCM` ('lpcm').
const AUDIO_FORMAT_LPCM: u32 = 0x6C70_636D;

/// An audio input device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicDevice {
    /// `AVCaptureDevice.uniqueID` (stable across launches).
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicPermission {
    Granted,
    Denied,
    NotDetermined,
}

/// Connected microphones: built-in / Continuity mics first, then external
/// (USB, Bluetooth, aggregate) devices, via `AVCaptureDeviceDiscoverySession`
/// (the `Microphone` + `External` device types are macOS 14+, our minimum).
pub fn devices() -> Vec<MicDevice> {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else { return Vec::new() };
    // SAFETY: device-type constants exist on macOS 14+; the discovery session
    // is created and read on this thread with valid arguments.
    let list = unsafe {
        let types = NSArray::from_slice(&[AVCaptureDeviceTypeMicrophone, AVCaptureDeviceTypeExternal]);
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            Some(audio),
            AVCaptureDevicePosition::Unspecified,
        )
        .devices()
    };
    let mut out: Vec<MicDevice> = Vec::new();
    for d in list.iter() {
        // SAFETY: read-only properties of a valid device.
        let dev = unsafe { MicDevice { id: d.uniqueID().to_string(), name: d.localizedName().to_string() } };
        if !out.iter().any(|m| m.id == dev.id) {
            out.push(dev);
        }
    }
    out
}

/// Current microphone (TCC) authorisation.
pub fn permission() -> MicPermission {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else { return MicPermission::Denied };
    // SAFETY: class method with a valid media type.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::Authorized => MicPermission::Granted,
        AVAuthorizationStatus::NotDetermined => MicPermission::NotDetermined,
        _ => MicPermission::Denied,
    }
}

/// Ask for microphone access (shows the system prompt the first time).
/// `done` runs on an arbitrary queue.
pub fn request_permission(done: impl FnOnce(bool) + Send + 'static) {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else { return done(false) };
    let slot = std::sync::Mutex::new(Some(done));
    let block = RcBlock::new(move |granted: Bool| {
        if let Some(f) = slot.lock().ok().and_then(|mut s| s.take()) {
            f(granted.as_bool());
        }
    });
    // SAFETY: valid media type and block.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &block) };
}

/// Whether ScreenCaptureKit can capture the microphone (macOS 15+).
pub fn sc_microphone_supported() -> bool {
    // SAFETY: creating an empty configuration and probing a selector.
    unsafe { SCStreamConfiguration::new().respondsToSelector(sel!(setCaptureMicrophone:)) }
}

type SampleHandler = Box<dyn Fn(&CMSampleBuffer) + Send + Sync>;

pub struct MicOutputIvars {
    handler: SampleHandler,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "LumoraMicOutput"]
    #[ivars = MicOutputIvars]
    struct MicOutput;

    unsafe impl NSObjectProtocol for MicOutput {}

    unsafe impl AVCaptureAudioDataOutputSampleBufferDelegate for MicOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            (self.ivars().handler)(sample);
        }
    }
);

impl MicOutput {
    fn new(handler: SampleHandler) -> Retained<Self> {
        let this = Self::alloc().set_ivars(MicOutputIvars { handler });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// Converts capture-session timestamps to the host clock.
pub(crate) struct HostClock {
    /// The session's synchronisation clock; `None` = already host time.
    source: Option<SendBox<Retained<CMClock>>>,
    host: SendBox<objc2_core_foundation::CFRetained<CMClock>>,
}

impl HostClock {
    /// PTS (host clock) of `sample`, retimed onto the host clock if needed.
    /// Returns the buffer to append (the original when no retiming was
    /// necessary).
    pub(crate) fn to_host(&self, sample: &CMSampleBuffer) -> Option<(f64, f64, Retained<CMSampleBuffer>)> {
        // SAFETY: CoreMedia accessors and conversions on valid objects.
        unsafe {
            let pts = sample.presentation_time_stamp();
            let dur = sample.duration().seconds();
            let dur = if dur.is_finite() { dur } else { 0.0 };
            let Some(source) = &self.source else {
                return Some((pts.seconds(), dur, Retained::retain(sample as *const _ as *mut CMSampleBuffer)?));
            };
            let host_pts = CMSyncConvertTime(pts, source, &self.host);
            let delta = host_pts.subtract(pts);
            let mut needed: objc2_core_media::CMItemCount = 0;
            sample.sample_timing_info_array(0, std::ptr::null_mut(), &mut needed);
            if needed <= 0 {
                return None;
            }
            let mut timing = vec![
                CMSampleTimingInfo {
                    duration: CMTime::new(0, 1),
                    presentationTimeStamp: CMTime::new(0, 1),
                    decodeTimeStamp: CMTime::new(0, 1),
                };
                needed as usize
            ];
            if sample.sample_timing_info_array(needed, timing.as_mut_ptr(), &mut needed) != 0 {
                return None;
            }
            for t in &mut timing {
                t.presentationTimeStamp = t.presentationTimeStamp.add(delta);
                t.decodeTimeStamp = objc2_core_media::kCMTimeInvalid;
            }
            let mut out: *mut CMSampleBuffer = std::ptr::null_mut();
            let status = CMSampleBuffer::create_copy_with_new_timing(
                None,
                sample,
                timing.len() as objc2_core_media::CMItemCount,
                timing.as_ptr(),
                NonNull::from(&mut out),
            );
            if status != 0 || out.is_null() {
                return None;
            }
            // +1 from the Create function.
            let retimed = Retained::from_raw(out)?;
            Some((host_pts.seconds(), dur, retimed))
        }
    }
}

/// A running `AVCaptureSession` feeding microphone samples to a handler.
pub(crate) struct MicSession {
    session: SendBox<Retained<AVCaptureSession>>,
    _output: SendBox<Retained<AVCaptureAudioDataOutput>>,
    _delegate: SendBox<Retained<MicOutput>>,
}

impl MicSession {
    /// Build and start a session on `device_id` (or the default mic),
    /// delivering 48 kHz stereo float LPCM on `queue`. The handler gets the
    /// raw buffer plus a [`HostClock`] to retime it.
    pub(crate) fn start(
        device_id: Option<&str>,
        queue: &DispatchQueue,
        handler: impl Fn(&CMSampleBuffer, &HostClock) + Send + Sync + 'static,
    ) -> Result<MicSession, CaptureError> {
        let audio =
            unsafe { AVMediaTypeAudio }.ok_or(CaptureError::Framework("AVMediaTypeAudio unavailable".into()))?;
        if permission() == MicPermission::Denied {
            return Err(CaptureError::MicrophoneDenied);
        }
        // SAFETY: AVFoundation object construction with valid arguments;
        // the session is configured before it starts and only started /
        // stopped afterwards (AVCaptureSession is thread-safe for that).
        unsafe {
            let device = device_id
                .and_then(|id| AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(id)))
                .or_else(|| AVCaptureDevice::defaultDeviceWithMediaType(audio))
                .ok_or(CaptureError::TargetGone("microphone"))?;
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
                .map_err(|e| CaptureError::Framework(e.localizedDescription().to_string()))?;
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            if !session.canAddInput(&input) {
                session.commitConfiguration();
                return Err(CaptureError::Framework("cannot add microphone input".into()));
            }
            session.addInput(&input);
            let output = AVCaptureAudioDataOutput::new();
            let fmt = NSNumber::numberWithUnsignedInt(AUDIO_FORMAT_LPCM);
            let rate = NSNumber::numberWithDouble(48_000.0);
            let channels = NSNumber::numberWithUnsignedInt(2);
            let bits = NSNumber::numberWithUnsignedInt(32);
            let yes = NSNumber::numberWithBool(true);
            let no = NSNumber::numberWithBool(false);
            let settings = NSDictionary::<NSString, objc2::runtime::AnyObject>::from_slices(
                &[
                    &*NSString::from_str("AVFormatIDKey"),
                    &*NSString::from_str("AVSampleRateKey"),
                    &*NSString::from_str("AVNumberOfChannelsKey"),
                    &*NSString::from_str("AVLinearPCMBitDepthKey"),
                    &*NSString::from_str("AVLinearPCMIsFloatKey"),
                    &*NSString::from_str("AVLinearPCMIsNonInterleaved"),
                ],
                &[&*fmt, &*rate, &*channels, &*bits, &*yes, &*no],
            );
            output.setAudioSettings(Some(&settings));
            if !session.canAddOutput(&output) {
                session.commitConfiguration();
                return Err(CaptureError::Framework("cannot add microphone output".into()));
            }
            session.addOutput(&output);
            session.commitConfiguration();

            let source = if session.respondsToSelector(sel!(synchronizationClock)) {
                session.synchronizationClock()
            } else {
                #[allow(deprecated)]
                session.masterClock()
            };
            let clock =
                HostClock { source: source.map(|c| SendBox::new(c)), host: SendBox::new(CMClock::host_time_clock()) };
            let delegate = MicOutput::new(Box::new(move |sample| handler(sample, &clock)));
            let proto: &ProtocolObject<dyn AVCaptureAudioDataOutputSampleBufferDelegate> =
                ProtocolObject::from_ref(&*delegate);
            output.setSampleBufferDelegate_queue(Some(proto), Some(queue));
            session.startRunning();
            Ok(MicSession {
                session: SendBox::new(session),
                _output: SendBox::new(output),
                _delegate: SendBox::new(delegate),
            })
        }
    }

    pub(crate) fn stop(&self) {
        // SAFETY: stopRunning is safe from any thread; it blocks until the
        // session has stopped, after which no more samples are delivered.
        unsafe { self.session.stopRunning() };
    }
}

impl Drop for MicSession {
    fn drop(&mut self) {
        self.stop();
    }
}
