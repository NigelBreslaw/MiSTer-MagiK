// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{CaptureVisibility, EncodedFrame, JPEG_HEIGHT, JPEG_WIDTH, analyze_luma, classified};
use crate::error::AgentResult;
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDeviceFormat, AVCaptureDeviceInput, AVCaptureDevicePosition, AVCaptureDeviceType,
    AVCaptureDeviceTypeExternal, AVCaptureFileOutput, AVCaptureFileOutputRecordingDelegate,
    AVCaptureMovieFileOutput, AVCaptureOutput, AVCaptureSession, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVFrameRateRange, AVMediaTypeVideo,
};
use objc2_core_graphics::CGColorSpace;
use objc2_core_image::{
    CIContext, CIImage, CIImageRepresentationOption, kCIContextUseSoftwareRenderer,
};
use objc2_core_media::{CMSampleBuffer, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::{
    CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType, CVPixelBufferGetPlaneCount,
    CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
};
use objc2_foundation::{
    NSArray, NSDate, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSRunLoop, NSURL,
    ns_string,
};
use std::path::Path;
use std::slice;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

const DEVICE_NAME: &str = "USB Video";
const REQUESTED_RATE: f64 = 30.0;
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30);
const MOVIE_START_TIMEOUT: Duration = Duration::from_secs(10);
const MOVIE_COMPLETION_TIMEOUT: Duration = Duration::from_secs(10);

struct DelegateIvars {
    result: Mutex<Option<SyncSender<AgentResult<EncodedFrame>>>>,
    require_visible: bool,
}

define_class!(
    // SAFETY: NSObject has no special subclassing requirements. The delegate owns only a
    // synchronized Rust sender and implements exactly the AVFoundation callback protocol.
    #[unsafe(super(NSObject))]
    #[ivars = DelegateIvars]
    struct FrameDelegate;

    unsafe impl NSObjectProtocol for FrameDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for FrameDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn capture_output(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            let result = process_sample(sample_buffer);
            let should_send = match &result {
                Ok(Some(frame)) => {
                    !self.ivars().require_visible
                        || frame.luma.is_some_and(|analysis| {
                            matches!(
                                analysis.visibility,
                                CaptureVisibility::Visible | CaptureVisibility::Corrupted
                            )
                        })
                }
                Ok(None) => false,
                Err(_) => true,
            };
            if should_send && let Some(sender) = self.ivars().result.lock().unwrap().take() {
                let _ = sender.send(
                    result.map(|frame| frame.expect("nonblank sample result must contain a frame")),
                );
            }
        }
    }
);

impl FrameDelegate {
    fn new(sender: SyncSender<AgentResult<EncodedFrame>>, require_visible: bool) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars {
            result: Mutex::new(Some(sender)),
            require_visible,
        });
        // SAFETY: The instance variables are initialized and NSObject's initializer is valid.
        unsafe { msg_send![super(this), init] }
    }
}

struct MovieDelegateIvars {
    events: Mutex<Option<SyncSender<MovieEvent>>>,
}

enum MovieEvent {
    Started,
    Finished(AgentResult<()>),
}

define_class!(
    // SAFETY: NSObject has no special subclassing requirements. The delegate owns only a
    // synchronized Rust sender and implements the required AVFoundation completion callback.
    #[unsafe(super(NSObject))]
    #[ivars = MovieDelegateIvars]
    struct MovieDelegate;

    unsafe impl NSObjectProtocol for MovieDelegate {}

    unsafe impl AVCaptureFileOutputRecordingDelegate for MovieDelegate {
        #[unsafe(method(captureOutput:didStartRecordingToOutputFileAtURL:fromConnections:))]
        unsafe fn capture_output_did_start_recording(
            &self,
            _output: &AVCaptureFileOutput,
            _output_file_url: &NSURL,
            _connections: &NSArray<AVCaptureConnection>,
        ) {
            if let Some(sender) = self.ivars().events.lock().unwrap().as_ref() {
                let _ = sender.send(MovieEvent::Started);
            }
        }

        #[unsafe(method(captureOutput:didFinishRecordingToOutputFileAtURL:fromConnections:error:))]
        unsafe fn capture_output_did_finish_recording(
            &self,
            _output: &AVCaptureFileOutput,
            _output_file_url: &NSURL,
            _connections: &NSArray<AVCaptureConnection>,
            error: Option<&NSError>,
        ) {
            let result = error.map_or_else(
                || Ok(()),
                |error| {
                    Err(classified(
                        "camera_recording_failed",
                        error.localizedDescription().to_string(),
                    ))
                },
            );
            if let Some(sender) = self.ivars().events.lock().unwrap().take() {
                let _ = sender.send(MovieEvent::Finished(result));
            }
        }
    }
);

impl MovieDelegate {
    fn new(sender: SyncSender<MovieEvent>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(MovieDelegateIvars {
            events: Mutex::new(Some(sender)),
        });
        // SAFETY: The instance variables are initialized and NSObject's initializer is valid.
        unsafe { msg_send![super(this), init] }
    }
}

fn wait_for_movie_start(receiver: &Receiver<MovieEvent>) -> AgentResult<()> {
    match receive_movie_event(receiver, MOVIE_START_TIMEOUT) {
        Ok(MovieEvent::Started) => Ok(()),
        Ok(MovieEvent::Finished(Err(error))) => Err(error),
        Ok(MovieEvent::Finished(Ok(()))) => Err(classified(
            "camera_recording_failed",
            "USB Video movie finished before recording started",
        )),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(classified(
            "camera_timeout",
            format!(
                "USB Video movie did not start within {} seconds",
                MOVIE_START_TIMEOUT.as_secs()
            ),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(classified(
            "camera_recording_failed",
            "USB Video movie callback disconnected before recording started",
        )),
    }
}

fn wait_for_movie_completion(receiver: &Receiver<MovieEvent>) -> AgentResult<()> {
    match receive_movie_event(receiver, MOVIE_COMPLETION_TIMEOUT) {
        Ok(MovieEvent::Finished(result)) => result,
        Ok(MovieEvent::Started) => Err(classified(
            "camera_recording_failed",
            "USB Video movie emitted a duplicate recording-start event",
        )),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(classified(
            "camera_timeout",
            format!(
                "USB Video movie did not finish writing within {} seconds",
                MOVIE_COMPLETION_TIMEOUT.as_secs()
            ),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(classified(
            "camera_recording_failed",
            "USB Video movie callback disconnected before recording finished",
        )),
    }
}

fn receive_movie_event(
    receiver: &Receiver<MovieEvent>,
    timeout: Duration,
) -> Result<MovieEvent, mpsc::RecvTimeoutError> {
    let deadline = Instant::now() + timeout;
    let run_loop = NSRunLoop::currentRunLoop();
    loop {
        match receiver.try_recv() {
            Ok(event) => return Ok(event),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(mpsc::RecvTimeoutError::Timeout);
        }
        let slice = remaining.min(Duration::from_millis(10));
        let limit = NSDate::dateWithTimeIntervalSinceNow(slice.as_secs_f64());
        run_loop.runUntilDate(&limit);
    }
}

pub(super) fn capture(timeout: Duration) -> AgentResult<EncodedFrame> {
    autoreleasepool(|_| capture_inner(timeout, true))
}

pub(super) fn capture_analyzed(timeout: Duration) -> AgentResult<EncodedFrame> {
    autoreleasepool(|_| capture_inner(timeout, false))
}

pub(super) fn record(output: &Path, duration: Duration) -> AgentResult<()> {
    autoreleasepool(|_| record_inner(output, duration))
}

fn capture_inner(timeout: Duration, require_visible: bool) -> AgentResult<EncodedFrame> {
    require_camera_access()?;
    let device = find_device()?;
    let (format, rate) = select_format(&device)?;
    configure_device(&device, &format, &rate)?;

    // SAFETY: All AVFoundation objects remain retained until the session is stopped below.
    let session = unsafe { AVCaptureSession::new() };
    let input =
        unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }.map_err(|error| {
            classified(
                "camera_unavailable",
                error.localizedDescription().to_string(),
            )
        })?;
    let output = unsafe { AVCaptureVideoDataOutput::new() };
    unsafe {
        output.setAlwaysDiscardsLateVideoFrames(true);
    }
    let pixel_format =
        NSNumber::numberWithUnsignedInt(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
    let settings = NSDictionary::<_, AnyObject>::from_slices(
        &[ns_string!("PixelFormatType")],
        &[pixel_format.as_ref()],
    );
    unsafe {
        output.setVideoSettings(Some(&settings));
        session.beginConfiguration();
    }
    if !unsafe { session.canAddInput(input.as_ref()) } {
        unsafe { session.commitConfiguration() };
        return Err(classified(
            "camera_configuration_failed",
            "cannot add USB Video input to capture session",
        ));
    }
    unsafe { session.addInput(input.as_ref()) };
    if !unsafe { session.canAddOutput(output.as_ref()) } {
        unsafe { session.commitConfiguration() };
        return Err(classified(
            "camera_configuration_failed",
            "cannot add frame output to capture session",
        ));
    }
    unsafe {
        session.addOutput(output.as_ref());
        session.commitConfiguration();
    }

    let (sender, receiver) = mpsc::sync_channel(1);
    let delegate = FrameDelegate::new(sender, require_visible);
    let queue = DispatchQueue::new(
        "io.mister-magik.agent-cli.usb-video",
        DispatchQueueAttr::SERIAL,
    );
    let protocol = ProtocolObject::from_ref(&*delegate);
    unsafe {
        output.setSampleBufferDelegate_queue(Some(protocol), Some(&queue));
        session.startRunning();
    }
    let result = receiver.recv_timeout(timeout).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => classified(
            "camera_timeout",
            format!(
                "USB Video did not produce a {} frame within {} seconds",
                if require_visible { "visible" } else { "valid" },
                timeout.as_secs()
            ),
        ),
        mpsc::RecvTimeoutError::Disconnected => classified(
            "camera_capture_failed",
            "USB Video frame callback disconnected",
        ),
    });
    unsafe {
        output.setSampleBufferDelegate_queue(None, None);
        session.stopRunning();
    }
    result?
}

fn record_inner(output_path: &Path, duration: Duration) -> AgentResult<()> {
    require_camera_access()?;
    let device = find_device()?;
    let (format, rate) = select_format(&device)?;
    configure_device(&device, &format, &rate)?;

    // SAFETY: All AVFoundation objects remain retained until recording has finished.
    let session = unsafe { AVCaptureSession::new() };
    let input =
        unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }.map_err(|error| {
            classified(
                "camera_unavailable",
                error.localizedDescription().to_string(),
            )
        })?;
    let output = unsafe { AVCaptureMovieFileOutput::new() };
    unsafe { session.beginConfiguration() };
    if !unsafe { session.canAddInput(input.as_ref()) } {
        unsafe { session.commitConfiguration() };
        return Err(classified(
            "camera_configuration_failed",
            "cannot add USB Video input to movie capture session",
        ));
    }
    unsafe { session.addInput(input.as_ref()) };
    if !unsafe { session.canAddOutput(output.as_ref()) } {
        unsafe { session.commitConfiguration() };
        return Err(classified(
            "camera_configuration_failed",
            "cannot add movie output to USB Video capture session",
        ));
    }
    unsafe {
        session.addOutput(output.as_ref());
        session.commitConfiguration();
    }

    let output_url = NSURL::from_file_path(output_path).ok_or_else(|| {
        classified(
            "camera_output_invalid",
            format!("cannot create a file URL for {}", output_path.display()),
        )
    })?;
    let (sender, receiver) = mpsc::sync_channel(2);
    let delegate = MovieDelegate::new(sender);
    let protocol = ProtocolObject::from_ref(&*delegate);
    unsafe {
        session.startRunning();
        output.startRecordingToOutputFileURL_recordingDelegate(&output_url, protocol);
    }

    let result = wait_for_movie_start(&receiver).and_then(|()| {
        std::thread::sleep(duration);
        // SAFETY: The delegate's start event proves this output accepted the recording.
        // Request its asynchronous finalization directly; a later property snapshot must
        // not be allowed to suppress the matching stop request.
        unsafe { output.stopRecording() };
        wait_for_movie_completion(&receiver)
    });
    if result.is_err() && unsafe { output.isRecording() } {
        unsafe { output.stopRecording() };
    }
    unsafe { session.stopRunning() };
    result
}

fn require_camera_access() -> AgentResult<()> {
    let media_type = unsafe { AVMediaTypeVideo }.ok_or_else(|| {
        classified(
            "camera_configuration_failed",
            "AVFoundation video media type is unavailable",
        )
    })?;
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    match status {
        AVAuthorizationStatus::Authorized => Ok(()),
        AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => Err(classified(
            "camera_access_denied",
            "camera access is denied; allow camera access for the invoking terminal or Codex",
        )),
        AVAuthorizationStatus::NotDetermined => {
            let (sender, receiver) = mpsc::sync_channel(1);
            let handler = RcBlock::new(move |granted: Bool| {
                let _ = sender.send(granted.as_bool());
            });
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler);
            }
            match receiver.recv_timeout(PERMISSION_TIMEOUT) {
                Ok(true) => Ok(()),
                Ok(false) => Err(classified(
                    "camera_access_denied",
                    "camera access was not granted",
                )),
                Err(_) => Err(classified(
                    "camera_permission_timeout",
                    "camera permission request did not complete within 30 seconds",
                )),
            }
        }
        _ => Err(classified(
            "camera_access_denied",
            "camera authorization returned an unknown status",
        )),
    }
}

fn find_device() -> AgentResult<Retained<AVCaptureDevice>> {
    let media_type = unsafe { AVMediaTypeVideo }.ok_or_else(|| {
        classified(
            "camera_configuration_failed",
            "AVFoundation video media type is unavailable",
        )
    })?;
    let external = unsafe { AVCaptureDeviceTypeExternal };
    let types = NSArray::<AVCaptureDeviceType>::from_slice(&[external]);
    let discovery = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            Some(media_type),
            AVCaptureDevicePosition::Unspecified,
        )
    };
    let matches: Vec<_> = unsafe { discovery.devices() }
        .to_vec()
        .into_iter()
        .filter(|device| unsafe { device.localizedName() }.to_string() == DEVICE_NAME)
        .collect();
    match matches.as_slice() {
        [] => Err(classified(
            "camera_unavailable",
            format!("camera named {DEVICE_NAME:?} was not found"),
        )),
        [device] => Ok(device.clone()),
        devices => Err(classified(
            "camera_ambiguous",
            format!(
                "found {} cameras named {DEVICE_NAME:?}; disconnect duplicate devices",
                devices.len()
            ),
        )),
    }
}

fn select_format(
    device: &AVCaptureDevice,
) -> AgentResult<(Retained<AVCaptureDeviceFormat>, Retained<AVFrameRateRange>)> {
    for format in unsafe { device.formats() }.to_vec() {
        let description = unsafe { format.formatDescription() };
        let dimensions = unsafe { CMVideoFormatDescriptionGetDimensions(&description) };
        if dimensions.width != i32::try_from(JPEG_WIDTH).unwrap()
            || dimensions.height != i32::try_from(JPEG_HEIGHT).unwrap()
            || unsafe { description.media_sub_type() }
                != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        {
            continue;
        }
        for rate in unsafe { format.videoSupportedFrameRateRanges() }.to_vec() {
            let minimum = unsafe { rate.minFrameRate() };
            let maximum = unsafe { rate.maxFrameRate() };
            if REQUESTED_RATE >= minimum - 0.01 && REQUESTED_RATE <= maximum + 0.01 {
                return Ok((format, rate));
            }
        }
    }
    Err(classified(
        "camera_format_unsupported",
        "USB Video does not provide native NV12 1920x1080 at 30 fps",
    ))
}

fn configure_device(
    device: &AVCaptureDevice,
    format: &AVCaptureDeviceFormat,
    rate: &AVFrameRateRange,
) -> AgentResult<()> {
    unsafe { device.lockForConfiguration() }.map_err(|error| {
        classified(
            "camera_configuration_failed",
            error.localizedDescription().to_string(),
        )
    })?;
    unsafe {
        device.setActiveFormat(format);
        device.setActiveVideoMinFrameDuration(rate.minFrameDuration());
        device.setActiveVideoMaxFrameDuration(rate.maxFrameDuration());
        device.unlockForConfiguration();
    }
    Ok(())
}

fn process_sample(sample_buffer: &CMSampleBuffer) -> AgentResult<Option<EncodedFrame>> {
    let pixel_buffer = unsafe { sample_buffer.image_buffer() }.ok_or_else(|| {
        classified(
            "camera_frame_invalid",
            "USB Video sample did not contain a pixel buffer",
        )
    })?;
    let width = CVPixelBufferGetWidth(&pixel_buffer);
    let height = CVPixelBufferGetHeight(&pixel_buffer);
    if width != usize::try_from(JPEG_WIDTH).unwrap()
        || height != usize::try_from(JPEG_HEIGHT).unwrap()
        || CVPixelBufferGetPixelFormatType(&pixel_buffer)
            != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        || CVPixelBufferGetPlaneCount(&pixel_buffer) < 2
        || CVPixelBufferGetWidthOfPlane(&pixel_buffer, 0) != width
        || CVPixelBufferGetHeightOfPlane(&pixel_buffer, 0) != height
    {
        return Err(classified(
            "camera_frame_invalid",
            "USB Video returned an unexpected pixel-buffer layout",
        ));
    }
    let flags = CVPixelBufferLockFlags::ReadOnly;
    let lock = unsafe { CVPixelBufferLockBaseAddress(&pixel_buffer, flags) };
    if lock != kCVReturnSuccess {
        return Err(classified(
            "camera_frame_invalid",
            format!("could not lock USB Video pixel buffer ({lock})"),
        ));
    }
    let row_bytes = CVPixelBufferGetBytesPerRowOfPlane(&pixel_buffer, 0);
    let base = CVPixelBufferGetBaseAddressOfPlane(&pixel_buffer, 0).cast::<u8>();
    let plane_len = row_bytes.checked_mul(height).ok_or_else(|| {
        classified(
            "camera_frame_invalid",
            "USB Video luma plane size overflowed",
        )
    })?;
    let analysis = if base.is_null() {
        None
    } else {
        // SAFETY: AVFoundation owns a locked luma plane of at least row_bytes * height bytes.
        let luma = unsafe { slice::from_raw_parts(base, plane_len) };
        analyze_luma(luma, width, height, row_bytes)
    };
    let unlock = unsafe { CVPixelBufferUnlockBaseAddress(&pixel_buffer, flags) };
    if unlock != kCVReturnSuccess {
        return Err(classified(
            "camera_frame_invalid",
            format!("could not unlock USB Video pixel buffer ({unlock})"),
        ));
    }
    if analysis.is_none() {
        return Ok(None);
    }

    let software = NSNumber::numberWithBool(true);
    let software_renderer = unsafe { kCIContextUseSoftwareRenderer };
    let context_options =
        NSDictionary::<_, AnyObject>::from_slices(&[software_renderer], &[software.as_ref()]);
    let context = unsafe { CIContext::contextWithOptions(Some(&context_options)) };
    let image = unsafe { CIImage::imageWithCVPixelBuffer(&pixel_buffer) };
    let color_space = CGColorSpace::new_device_rgb().ok_or_else(|| {
        classified(
            "camera_encode_failed",
            "could not create an RGB color space",
        )
    })?;
    let quality = NSNumber::numberWithDouble(0.9);
    let representation_options =
        NSDictionary::<CIImageRepresentationOption, AnyObject>::from_slices(
            &[ns_string!("kCGImageDestinationLossyCompressionQuality")],
            &[quality.as_ref()],
        );
    let data = unsafe {
        context.JPEGRepresentationOfImage_colorSpace_options(
            &image,
            &color_space,
            &representation_options,
        )
    }
    .ok_or_else(|| classified("camera_encode_failed", "Core Image JPEG encoding failed"))?;
    Ok(Some(EncodedFrame {
        jpeg: data.to_vec(),
        width: JPEG_WIDTH,
        height: JPEG_HEIGHT,
        luma: analysis,
    }))
}
