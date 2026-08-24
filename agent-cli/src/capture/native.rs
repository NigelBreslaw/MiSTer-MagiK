// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{
    CaptureVisibility, EncodedFrame, JPEG_HEIGHT, JPEG_WIDTH, MovieObservation, analyze_luma,
    classified, validate_movie_observation,
};
use crate::error::{AgentError, AgentResult};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAsset, AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetWriter,
    AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDeviceFormat, AVCaptureDeviceInput, AVCaptureDevicePosition, AVCaptureDeviceType,
    AVCaptureDeviceTypeExternal, AVCaptureOutput, AVCaptureSession, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVFileTypeQuickTimeMovie, AVFrameRateRange,
    AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGColorSpace;
use objc2_core_image::{
    CIContext, CIImage, CIImageRepresentationOption, kCIContextUseSoftwareRenderer,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
    CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    kCVReturnSuccess,
};
use objc2_foundation::{
    NSArray, NSDate, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSRunLoop, NSURL,
    ns_string,
};
use std::path::Path;
use std::ptr::NonNull;
use std::slice;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

const DEVICE_NAME: &str = "USB Video";
const REQUESTED_RATE: f64 = 30.0;
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30);
const MOVIE_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
const MOVIE_COMPLETION_TIMEOUT: Duration = Duration::from_secs(10);
const MOVIE_FRAME_TIMEOUT: Duration = Duration::from_secs(3);
const MOVIE_FRAME_QUEUE_CAPACITY: usize = 4;

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

struct RawMoviePlane {
    bytes: Vec<u8>,
    row_bytes: usize,
    height: usize,
}

struct RawMovieFrame {
    captured_at: Instant,
    planes: [RawMoviePlane; 2],
}

struct MovieFrameDelegateIvars {
    frames: Mutex<Option<SyncSender<AgentResult<RawMovieFrame>>>>,
}

define_class!(
    // SAFETY: NSObject has no special subclassing requirements. The delegate copies each
    // delivered pixel buffer into Rust-owned bytes before crossing the dispatch boundary.
    #[unsafe(super(NSObject))]
    #[ivars = MovieFrameDelegateIvars]
    struct MovieFrameDelegate;

    unsafe impl NSObjectProtocol for MovieFrameDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for MovieFrameDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn capture_output(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            let result = copy_movie_sample(sample_buffer);
            let mut sender = self.ivars().frames.lock().unwrap();
            if let Some(channel) = sender.as_ref() {
                let connected = match result {
                    Ok(frame) => match channel.try_send(Ok(frame)) {
                        Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
                        Err(mpsc::TrySendError::Disconnected(_)) => false,
                    },
                    // A malformed native frame is a fail-closed recording error, not a frame
                    // eligible for real-time backpressure dropping.
                    Err(error) => channel.send(Err(error)).is_ok(),
                };
                if !connected {
                    *sender = None;
                }
            }
        }
    }
);

impl MovieFrameDelegate {
    fn new(sender: SyncSender<AgentResult<RawMovieFrame>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(MovieFrameDelegateIvars {
            frames: Mutex::new(Some(sender)),
        });
        // SAFETY: The instance variables are initialized and NSObject's initializer is valid.
        unsafe { msg_send![super(this), init] }
    }
}

fn copy_movie_sample(sample_buffer: &CMSampleBuffer) -> AgentResult<RawMovieFrame> {
    let pixel_buffer = unsafe { sample_buffer.image_buffer() }.ok_or_else(|| {
        classified(
            "camera_movie_frame_failed",
            "AVFoundation delivered a USB Video movie sample without a pixel buffer",
        )
    })?;
    let width = CVPixelBufferGetWidth(&pixel_buffer);
    let height = CVPixelBufferGetHeight(&pixel_buffer);
    let format = CVPixelBufferGetPixelFormatType(&pixel_buffer);
    let plane_count = CVPixelBufferGetPlaneCount(&pixel_buffer);
    if width != JPEG_WIDTH as usize
        || height != JPEG_HEIGHT as usize
        || format != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        || plane_count != 2
    {
        return Err(classified(
            "camera_movie_frame_failed",
            format!(
                "AVFoundation delivered USB Video movie frame {width}x{height} format=0x{format:08x} planes={plane_count}; expected {JPEG_WIDTH}x{JPEG_HEIGHT} NV12 with 2 planes"
            ),
        ));
    }
    let lock =
        unsafe { CVPixelBufferLockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    if lock != kCVReturnSuccess {
        return Err(classified(
            "camera_movie_frame_failed",
            format!(
                "CVPixelBufferLockBaseAddress failed for USB Video movie source frame with CVReturn {lock}"
            ),
        ));
    }
    let copied = (|| {
        let mut planes = Vec::with_capacity(2);
        for plane in 0..2 {
            let row_bytes = CVPixelBufferGetBytesPerRowOfPlane(&pixel_buffer, plane);
            let plane_height = CVPixelBufferGetHeightOfPlane(&pixel_buffer, plane);
            let base = CVPixelBufferGetBaseAddressOfPlane(&pixel_buffer, plane);
            let byte_count = row_bytes.checked_mul(plane_height).ok_or_else(|| {
                classified(
                    "camera_movie_frame_failed",
                    format!("USB Video movie source plane {plane} size overflowed"),
                )
            })?;
            if base.is_null() || byte_count == 0 {
                return Err(classified(
                    "camera_movie_frame_failed",
                    format!("USB Video movie source plane {plane} has no readable bytes"),
                ));
            }
            let expected_height = if plane == 0 {
                JPEG_HEIGHT as usize
            } else {
                JPEG_HEIGHT as usize / 2
            };
            if row_bytes < JPEG_WIDTH as usize || plane_height != expected_height {
                return Err(classified(
                    "camera_movie_frame_failed",
                    format!(
                        "USB Video movie source plane {plane} has row_bytes={row_bytes} height={plane_height}; expected at least {JPEG_WIDTH} row bytes and height {expected_height}"
                    ),
                ));
            }
            let bytes = unsafe { slice::from_raw_parts(base.cast::<u8>(), byte_count) }.to_vec();
            planes.push(RawMoviePlane {
                bytes,
                row_bytes,
                height: plane_height,
            });
        }
        let [first, second]: [RawMoviePlane; 2] = planes.try_into().map_err(|_| {
            classified(
                "camera_movie_frame_failed",
                "USB Video movie source did not yield exactly two copied NV12 planes",
            )
        })?;
        Ok(RawMovieFrame {
            captured_at: Instant::now(),
            planes: [first, second],
        })
    })();
    let unlock =
        unsafe { CVPixelBufferUnlockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    if unlock != kCVReturnSuccess {
        return Err(classified(
            "camera_movie_frame_failed",
            format!(
                "CVPixelBufferUnlockBaseAddress failed for USB Video movie source frame with CVReturn {unlock}"
            ),
        ));
    }
    copied
}

fn describe_avfoundation_error(stage: &str, error: &NSError) -> String {
    let mut detail = format!(
        "{stage} failed: domain={} code={} description={}",
        error.domain(),
        error.code(),
        error.localizedDescription()
    );
    if let Some(reason) = error.localizedFailureReason() {
        detail.push_str(&format!("; reason={reason}"));
    }
    if let Some(suggestion) = error.localizedRecoverySuggestion() {
        detail.push_str(&format!("; recovery={suggestion}"));
    }
    for (index, underlying) in error
        .underlyingErrors()
        .to_vec()
        .into_iter()
        .take(3)
        .enumerate()
    {
        detail.push_str(&format!(
            "; underlying_{}=domain:{} code:{} description:{}",
            index + 1,
            underlying.domain(),
            underlying.code(),
            underlying.localizedDescription()
        ));
        if let Some(reason) = underlying.localizedFailureReason() {
            detail.push_str(&format!(" reason:{reason}"));
        }
    }
    detail
}

fn receive_with_run_loop<T>(
    receiver: &Receiver<T>,
    timeout: Duration,
) -> Result<T, mpsc::RecvTimeoutError> {
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
    autoreleasepool(|_| {
        record_inner(output, duration)?;
        validate_movie(output, duration)
    })
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
                describe_avfoundation_error("AVCaptureDeviceInput initialization", &error),
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

    // SAFETY: All AVFoundation objects remain retained until recording and writer finalization
    // have finished.
    let session = unsafe { AVCaptureSession::new() };
    let input =
        unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }.map_err(|error| {
            classified(
                "camera_unavailable",
                describe_avfoundation_error("AVCaptureDeviceInput movie initialization", &error),
            )
        })?;
    let output = unsafe { AVCaptureVideoDataOutput::new() };
    unsafe { output.setAlwaysDiscardsLateVideoFrames(true) };
    let pixel_format =
        NSNumber::numberWithUnsignedInt(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
    let capture_settings = NSDictionary::<_, AnyObject>::from_slices(
        &[ns_string!("PixelFormatType")],
        &[pixel_format.as_ref()],
    );
    unsafe {
        output.setVideoSettings(Some(&capture_settings));
        session.beginConfiguration();
    }
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
            "cannot add native frame output to USB Video movie capture session",
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
    let file_type = unsafe { AVFileTypeQuickTimeMovie }.ok_or_else(|| {
        classified(
            "camera_movie_writer_configuration_failed",
            "AVFoundation QuickTime movie file type is unavailable",
        )
    })?;
    let writer =
        unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&output_url, file_type) }
            .map_err(|error| {
                classified(
                    "camera_movie_writer_configuration_failed",
                    describe_avfoundation_error("AVAssetWriter initialization", &error),
                )
            })?;
    let media_type = unsafe { AVMediaTypeVideo }.ok_or_else(|| {
        classified(
            "camera_movie_writer_configuration_failed",
            "AVFoundation video media type is unavailable for movie writing",
        )
    })?;
    let codec_key = unsafe { AVVideoCodecKey }.ok_or_else(|| {
        classified(
            "camera_movie_writer_configuration_failed",
            "AVFoundation video codec setting key is unavailable",
        )
    })?;
    let codec = unsafe { AVVideoCodecTypeH264 }.ok_or_else(|| {
        classified(
            "camera_movie_writer_configuration_failed",
            "AVFoundation H.264 codec type is unavailable",
        )
    })?;
    let width_key = unsafe { AVVideoWidthKey }.ok_or_else(|| {
        classified(
            "camera_movie_writer_configuration_failed",
            "AVFoundation video width setting key is unavailable",
        )
    })?;
    let height_key = unsafe { AVVideoHeightKey }.ok_or_else(|| {
        classified(
            "camera_movie_writer_configuration_failed",
            "AVFoundation video height setting key is unavailable",
        )
    })?;
    let width = NSNumber::numberWithUnsignedInt(JPEG_WIDTH);
    let height = NSNumber::numberWithUnsignedInt(JPEG_HEIGHT);
    let writer_settings = NSDictionary::<_, AnyObject>::from_slices(
        &[codec_key, width_key, height_key],
        &[codec.as_ref(), width.as_ref(), height.as_ref()],
    );
    let writer_input = unsafe {
        AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            media_type,
            Some(&writer_settings),
        )
    };
    unsafe { writer_input.setExpectsMediaDataInRealTime(true) };
    if !unsafe { writer.canAddInput(&writer_input) } {
        return Err(classified(
            "camera_movie_writer_configuration_failed",
            "AVAssetWriter rejected the H.264 USB Video input",
        ));
    }
    unsafe { writer.addInput(&writer_input) };
    let adaptor_attributes = NSDictionary::<_, AnyObject>::from_slices(
        &[
            ns_string!("PixelFormatType"),
            ns_string!("Width"),
            ns_string!("Height"),
        ],
        &[pixel_format.as_ref(), width.as_ref(), height.as_ref()],
    );
    let adaptor = unsafe {
        AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
            &writer_input,
            Some(&adaptor_attributes),
        )
    };
    if !unsafe { writer.startWriting() } {
        return Err(writer_failure(
            "camera_movie_writer_start_failed",
            "AVAssetWriter startWriting",
            &writer,
        ));
    }
    unsafe { writer.startSessionAtSourceTime(CMTime::new(0, 1_000_000)) };

    let (sender, receiver) = mpsc::sync_channel(MOVIE_FRAME_QUEUE_CAPACITY);
    let delegate = MovieFrameDelegate::new(sender);
    let protocol = ProtocolObject::from_ref(&*delegate);
    let queue = DispatchQueue::new(
        "io.mister-magik.agent-cli.usb-video-movie",
        DispatchQueueAttr::SERIAL,
    );
    unsafe {
        output.setSampleBufferDelegate_queue(Some(protocol), Some(&queue));
        session.startRunning();
    }
    let result = write_movie_frames(&receiver, duration, &writer, &writer_input, &adaptor);
    unsafe {
        output.setSampleBufferDelegate_queue(None, None);
        session.stopRunning();
    }
    if result.is_err() {
        unsafe { writer.cancelWriting() };
    }
    result
}

fn writer_failure(code: &'static str, stage: &str, writer: &AVAssetWriter) -> AgentError {
    let status = unsafe { writer.status() };
    let detail = unsafe { writer.error() }.map_or_else(
        || format!("{stage} failed with writer status {status:?} and no NSError"),
        |error| describe_avfoundation_error(stage, &error),
    );
    classified(code, detail)
}

fn write_movie_frames(
    receiver: &Receiver<AgentResult<RawMovieFrame>>,
    duration: Duration,
    writer: &AVAssetWriter,
    writer_input: &AVAssetWriterInput,
    adaptor: &AVAssetWriterInputPixelBufferAdaptor,
) -> AgentResult<()> {
    let first =
        receive_with_run_loop(receiver, MOVIE_FIRST_FRAME_TIMEOUT).map_err(
            |error| match error {
                mpsc::RecvTimeoutError::Timeout => classified(
                    "camera_movie_first_frame_timeout",
                    format!(
                        "USB Video did not deliver the first native movie frame within {} seconds",
                        MOVIE_FIRST_FRAME_TIMEOUT.as_secs()
                    ),
                ),
                mpsc::RecvTimeoutError::Disconnected => classified(
                    "camera_movie_frame_callback_disconnected",
                    "USB Video native movie frame callback disconnected before the first frame",
                ),
            },
        )??;
    let started_at = first.captured_at;
    let mut next = Some(first);
    let mut appended_frames = 0_u64;
    let final_pts = loop {
        let frame = if let Some(frame) = next.take() {
            frame
        } else {
            receive_with_run_loop(receiver, MOVIE_FRAME_TIMEOUT).map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => classified(
                    "camera_movie_frame_timeout",
                    format!(
                        "USB Video stopped delivering native movie frames for {} seconds after {appended_frames} appended frames",
                        MOVIE_FRAME_TIMEOUT.as_secs()
                    ),
                ),
                mpsc::RecvTimeoutError::Disconnected => classified(
                    "camera_movie_frame_callback_disconnected",
                    format!(
                        "USB Video native movie frame callback disconnected after {appended_frames} appended frames"
                    ),
                ),
            })??
        };
        let elapsed = frame.captured_at.saturating_duration_since(started_at);
        if unsafe { writer_input.isReadyForMoreMediaData() } {
            let micros = elapsed.as_micros().min(i64::MAX as u128) as i64;
            let pts = unsafe { CMTime::new(micros, 1_000_000) };
            append_movie_frame(&frame, pts, writer, adaptor)?;
            appended_frames = appended_frames.saturating_add(1);
            if elapsed >= duration {
                break pts;
            }
        } else {
            let status = unsafe { writer.status() };
            if status != AVAssetWriterStatus::Writing {
                return Err(writer_failure(
                    "camera_movie_writer_append_failed",
                    "AVAssetWriter backpressure",
                    writer,
                ));
            }
        }
    };
    if appended_frames < 2 {
        return Err(classified(
            "camera_movie_writer_append_failed",
            format!("AVAssetWriter accepted only {appended_frames} USB Video movie frame(s)"),
        ));
    }
    unsafe {
        writer_input.markAsFinished();
        writer.endSessionAtSourceTime(final_pts);
    }
    let (sender, completion) = mpsc::sync_channel(1);
    let handler = RcBlock::new(move || {
        let _ = sender.send(());
    });
    unsafe { writer.finishWritingWithCompletionHandler(&handler) };
    receive_with_run_loop(&completion, MOVIE_COMPLETION_TIMEOUT).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => classified(
            "camera_movie_writer_finish_timeout",
            format!(
                "AVAssetWriter did not finish the USB Video movie within {} seconds after {appended_frames} appended frames",
                MOVIE_COMPLETION_TIMEOUT.as_secs()
            ),
        ),
        mpsc::RecvTimeoutError::Disconnected => classified(
            "camera_movie_writer_finish_disconnected",
            format!(
                "AVAssetWriter completion callback disconnected after {appended_frames} appended frames"
            ),
        ),
    })?;
    if unsafe { writer.status() } != AVAssetWriterStatus::Completed {
        return Err(writer_failure(
            "camera_movie_writer_finish_failed",
            "AVAssetWriter finishWriting",
            writer,
        ));
    }
    Ok(())
}

fn append_movie_frame(
    frame: &RawMovieFrame,
    pts: CMTime,
    writer: &AVAssetWriter,
    adaptor: &AVAssetWriterInputPixelBufferAdaptor,
) -> AgentResult<()> {
    let pool = unsafe { adaptor.pixelBufferPool() }.ok_or_else(|| {
        classified(
            "camera_movie_writer_buffer_failed",
            "AVAssetWriter did not create its configured NV12 pixel-buffer pool",
        )
    })?;
    let mut raw_buffer = std::ptr::null_mut();
    let output_pointer = NonNull::from(&mut raw_buffer);
    let create_status =
        unsafe { CVPixelBufferPool::create_pixel_buffer(None, &pool, output_pointer) };
    if create_status != kCVReturnSuccess {
        return Err(classified(
            "camera_movie_writer_buffer_failed",
            format!("CVPixelBufferPoolCreatePixelBuffer failed with CVReturn {create_status}"),
        ));
    }
    let raw_buffer = NonNull::new(raw_buffer).ok_or_else(|| {
        classified(
            "camera_movie_writer_buffer_failed",
            "CVPixelBufferPoolCreatePixelBuffer succeeded without returning a pixel buffer",
        )
    })?;
    let buffer: CFRetained<CVPixelBuffer> = unsafe { CFRetained::from_raw(raw_buffer) };
    let lock_flags = CVPixelBufferLockFlags::empty();
    let lock = unsafe { CVPixelBufferLockBaseAddress(&buffer, lock_flags) };
    if lock != kCVReturnSuccess {
        return Err(classified(
            "camera_movie_writer_buffer_failed",
            format!("CVPixelBufferLockBaseAddress failed for writer buffer with CVReturn {lock}"),
        ));
    }
    let copied = (|| {
        let width = CVPixelBufferGetWidth(&buffer);
        let height = CVPixelBufferGetHeight(&buffer);
        let format = CVPixelBufferGetPixelFormatType(&buffer);
        let plane_count = CVPixelBufferGetPlaneCount(&buffer);
        if width != JPEG_WIDTH as usize
            || height != JPEG_HEIGHT as usize
            || format != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
            || plane_count != 2
        {
            return Err(classified(
                "camera_movie_writer_buffer_failed",
                format!(
                    "AVAssetWriter supplied buffer {width}x{height} format=0x{format:08x} planes={plane_count}; expected {JPEG_WIDTH}x{JPEG_HEIGHT} NV12 with 2 planes"
                ),
            ));
        }
        for (plane, source) in frame.planes.iter().enumerate() {
            let destination_row_bytes = CVPixelBufferGetBytesPerRowOfPlane(&buffer, plane);
            let destination_height = CVPixelBufferGetHeightOfPlane(&buffer, plane);
            let destination = CVPixelBufferGetBaseAddressOfPlane(&buffer, plane);
            if destination.is_null()
                || destination_row_bytes < JPEG_WIDTH as usize
                || destination_height != source.height
            {
                return Err(classified(
                    "camera_movie_writer_buffer_failed",
                    format!(
                        "AVAssetWriter plane {plane} has row_bytes={destination_row_bytes} height={destination_height}; source has row_bytes={} height={} and requires at least {JPEG_WIDTH} active bytes",
                        source.row_bytes, source.height
                    ),
                ));
            }
            for row in 0..source.height {
                let source_offset = row * source.row_bytes;
                let destination_row =
                    unsafe { destination.cast::<u8>().add(row * destination_row_bytes) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        source.bytes.as_ptr().add(source_offset),
                        destination_row,
                        JPEG_WIDTH as usize,
                    );
                    if destination_row_bytes > JPEG_WIDTH as usize {
                        std::ptr::write_bytes(
                            destination_row.add(JPEG_WIDTH as usize),
                            0,
                            destination_row_bytes - JPEG_WIDTH as usize,
                        );
                    }
                }
            }
        }
        Ok(())
    })();
    let unlock = unsafe { CVPixelBufferUnlockBaseAddress(&buffer, lock_flags) };
    if unlock != kCVReturnSuccess {
        return Err(classified(
            "camera_movie_writer_buffer_failed",
            format!(
                "CVPixelBufferUnlockBaseAddress failed for writer buffer with CVReturn {unlock}"
            ),
        ));
    }
    copied?;
    if !unsafe { adaptor.appendPixelBuffer_withPresentationTime(&buffer, pts) } {
        return Err(writer_failure(
            "camera_movie_writer_append_failed",
            "AVAssetWriter pixel-buffer append",
            writer,
        ));
    }
    Ok(())
}

fn validate_movie(output_path: &Path, requested_duration: Duration) -> AgentResult<()> {
    let output_url = NSURL::from_file_path(output_path).ok_or_else(|| {
        classified(
            "camera_movie_validation_failed",
            format!(
                "AVFoundation could not create a file URL for recorded movie {}",
                output_path.display()
            ),
        )
    })?;
    let media_type = unsafe { AVMediaTypeVideo }.ok_or_else(|| {
        classified(
            "camera_movie_validation_failed",
            "AVFoundation video media type is unavailable during recorded-movie validation",
        )
    })?;
    let asset = unsafe { AVAsset::assetWithURL(&output_url) };
    // Validation is deliberately synchronous: the capture command must not publish the path
    // until every frame has decoded. macOS retains this compatibility accessor for that use.
    #[allow(deprecated)]
    let tracks = unsafe { asset.tracksWithMediaType(media_type) }.to_vec();
    let track = tracks.first().ok_or_else(|| {
        classified(
            "camera_movie_validation_failed",
            "AVFoundation found no video track in the recorded USB Video movie",
        )
    })?;
    let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }.map_err(|error| {
        classified(
            "camera_movie_validation_failed",
            describe_avfoundation_error("AVAssetReader initialization", &error),
        )
    })?;
    let pixel_format =
        NSNumber::numberWithUnsignedInt(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
    let settings = NSDictionary::<_, AnyObject>::from_slices(
        &[ns_string!("PixelFormatType")],
        &[pixel_format.as_ref()],
    );
    let output = unsafe {
        AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
            track,
            Some(&settings),
        )
    };
    if !unsafe { reader.canAddOutput(output.as_ref()) } {
        return Err(classified(
            "camera_movie_validation_failed",
            "AVAssetReader rejected the NV12 video-track decoder output",
        ));
    }
    unsafe { reader.addOutput(output.as_ref()) };
    if !unsafe { reader.startReading() } {
        let detail = unsafe { reader.error() }.map_or_else(
            || {
                format!(
                    "AVAssetReader startReading failed with status {:?} and no NSError",
                    unsafe { reader.status() }
                )
            },
            |error| describe_avfoundation_error("AVAssetReader startReading", &error),
        );
        return Err(classified("camera_movie_validation_failed", detail));
    }

    let mut frames = 0_u64;
    let mut first_pts = None;
    let mut last_pts = None;
    let mut width = 0_u32;
    let mut height = 0_u32;
    while let Some(sample) = unsafe { output.copyNextSampleBuffer() } {
        let pixel_buffer = unsafe { sample.image_buffer() }.ok_or_else(|| {
            classified(
                "camera_movie_validation_failed",
                format!("decoded USB Video frame {frames} has no pixel buffer"),
            )
        })?;
        let sample_width = CVPixelBufferGetWidth(&pixel_buffer);
        let sample_height = CVPixelBufferGetHeight(&pixel_buffer);
        width = sample_width.try_into().unwrap_or(u32::MAX);
        height = sample_height.try_into().unwrap_or(u32::MAX);
        if width != JPEG_WIDTH || height != JPEG_HEIGHT {
            return Err(classified(
                "camera_movie_validation_failed",
                format!(
                    "AVFoundation decoded frame {frames} at {width}x{height}; expected {JPEG_WIDTH}x{JPEG_HEIGHT}"
                ),
            ));
        }
        let pts = unsafe { sample.presentation_time_stamp().seconds() };
        if !pts.is_finite() {
            return Err(classified(
                "camera_movie_validation_failed",
                format!("decoded USB Video frame {frames} has non-finite presentation time {pts}"),
            ));
        }
        if let Some(previous) = last_pts
            && pts <= previous
        {
            return Err(classified(
                "camera_movie_validation_failed",
                format!(
                    "decoded USB Video frame {frames} presentation time {pts:.6}s did not advance beyond {previous:.6}s"
                ),
            ));
        }
        first_pts.get_or_insert(pts);
        last_pts = Some(pts);
        frames = frames.saturating_add(1);
    }
    let status = unsafe { reader.status() };
    if status != AVAssetReaderStatus::Completed {
        let detail = unsafe { reader.error() }.map_or_else(
            || format!("AVAssetReader stopped at status {status:?} with no NSError"),
            |error| describe_avfoundation_error("AVAssetReader frame decode", &error),
        );
        return Err(classified("camera_movie_validation_failed", detail));
    }
    let decoded_duration_seconds = match (first_pts, last_pts) {
        (Some(first), Some(last)) => last - first,
        _ => 0.0,
    };
    validate_movie_observation(
        MovieObservation {
            frames,
            width,
            height,
            decoded_duration_seconds,
        },
        requested_duration,
    )
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
            describe_avfoundation_error("USB Video device configuration lock", &error),
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
