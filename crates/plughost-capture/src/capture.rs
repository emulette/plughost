//! A ScreenCaptureKit stream of a region of a display, filtered to one helper's windows.

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once, PoisonError, Weak, mpsc};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, Message, define_class, msg_send};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType, SCWindow,
};
use plughost_core::remote::{Rect, Region};

use crate::errors::{NO_ANSWER, NO_DISPLAY, NO_HELPER, PERMISSION, REGION, START, STOPPED};

/// How long ScreenCaptureKit may take to answer a request.
const ANSWER: Duration = Duration::from_secs(10);
/// ScreenCaptureKit's error for a capture the user did not allow (`SCStreamErrorUserDeclined`).
const DECLINED: isize = -3801;
/// The status of a complete frame (`SCFrameStatusComplete`).
const COMPLETE: isize = 0;
/// How often the captures leave out other helpers' new windows.
const REFRESH: Duration = Duration::from_millis(500);

/// The pixels of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8-bit blue, green, red, alpha in one plane (`'BGRA'`).
    #[default]
    Bgra,
    /// 8-bit video-range luma and interleaved chroma in two planes (`'420v'`), which video
    /// encoders take without converting.
    Nv12,
}

impl PixelFormat {
    fn code(self) -> u32 {
        match self {
            PixelFormat::Bgra => u32::from_be_bytes(*b"BGRA"),
            PixelFormat::Nv12 => u32::from_be_bytes(*b"420v"),
        }
    }
}

/// How a capture runs.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct CaptureOptions {
    /// The most frames a second. Frames come only when the region changes.
    pub max_fps: f64,
    /// How many frames the stream keeps in flight. The application can hold two fewer
    /// [`Frame`]s at once; holding more stalls the stream.
    pub queue_depth: u32,
    /// The frames' pixels.
    pub pixel_format: PixelFormat,
}

impl Default for CaptureOptions {
    fn default() -> CaptureOptions {
        CaptureOptions {
            max_fps: 30.0,
            queue_depth: 5,
            pixel_format: PixelFormat::default(),
        }
    }
}

/// One complete frame of the region: its pixel buffer, at the display's backing scale.
pub struct Frame {
    buffer: CFRetained<CVPixelBuffer>,
    scale: f64,
    time: Duration,
}

// SAFETY: a pixel buffer is reference counted atomically and the frame only reads it.
unsafe impl Send for Frame {}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width())
            .field("height", &self.height())
            .field("scale", &self.scale)
            .field("time", &self.time)
            .finish()
    }
}

impl Frame {
    /// The frame's `CVPixelBufferRef`, valid while the frame lives. Retain it to keep it longer.
    pub fn pixel_buffer(&self) -> NonNull<c_void> {
        CFRetained::as_ptr(&self.buffer).cast()
    }

    /// The width in pixels.
    pub fn width(&self) -> usize {
        CVPixelBufferGetWidth(&self.buffer)
    }

    /// The height in pixels.
    pub fn height(&self) -> usize {
        CVPixelBufferGetHeight(&self.buffer)
    }

    /// Pixels per point.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// When the frame was captured, on the host's clock.
    pub fn time(&self) -> Duration {
        self.time
    }
}

/// Why a capture could not start or change.
#[derive(Debug)]
#[non_exhaustive]
pub enum CaptureError {
    /// The application has no screen recording permission.
    PermissionDenied,
    /// No display has the region's display identifier.
    NoDisplay,
    /// The helper has no window to capture yet.
    NoHelper,
    /// The region is not within its display, or a capture would move to another display.
    Region,
    /// ScreenCaptureKit failed with this error.
    Failed {
        /// The error's code.
        code: isize,
        /// The error's description.
        message: String,
    },
    /// ScreenCaptureKit did not answer in time.
    NoAnswer,
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::PermissionDenied => f.write_str(PERMISSION),
            CaptureError::NoDisplay => f.write_str(NO_DISPLAY),
            CaptureError::NoHelper => f.write_str(NO_HELPER),
            CaptureError::Region => f.write_str(REGION),
            CaptureError::Failed { code, message } => write!(f, "{START}: {message} ({code})"),
            CaptureError::NoAnswer => f.write_str(NO_ANSWER),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Why ScreenCaptureKit stopped a running capture.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StopReason {
    /// The application lost screen recording permission.
    PermissionDenied,
    /// ScreenCaptureKit stopped with this error.
    Failed {
        /// The error's code.
        code: isize,
        /// The error's description.
        message: String,
    },
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopReason::PermissionDenied => f.write_str(PERMISSION),
            StopReason::Failed { code, message } => write!(f, "{STOPPED}: {message} ({code})"),
        }
    }
}

/// The code and description of an error ScreenCaptureKit reported.
fn described(error: &NSError) -> (isize, String) {
    (error.code(), error.localizedDescription().to_string())
}

fn failed(error: &NSError) -> CaptureError {
    match described(error) {
        (DECLINED, _) => CaptureError::PermissionDenied,
        (code, message) => CaptureError::Failed { code, message },
    }
}

thread_local! {
    /// The capture whose callback runs on this thread, which a drop in it does not wait for.
    static CALLING: std::cell::Cell<*const Shared> = const { std::cell::Cell::new(std::ptr::null()) };
}

/// Runs `call` as a callback of `shared`'s capture.
fn calling(shared: &Arc<Shared>, call: impl FnOnce()) {
    let before = CALLING.replace(Arc::as_ptr(shared));
    call();
    CALLING.set(before);
}

/// What the stream's callbacks share with the capture.
struct Shared {
    on_frame: Box<dyn Fn(Frame) + Send + Sync>,
    on_stop: Box<dyn Fn(StopReason) + Send + Sync>,
    scale: f64,
    paused: AtomicBool,
    /// The capture is being dropped; nothing more reaches the application.
    ended: AtomicBool,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "PlughostCaptureOutput"]
    #[ivars = Arc<Shared>]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let shared = self.ivars();
            if shared.ended.load(Ordering::Acquire) {
                return;
            }
            let reason = match described(error) {
                (DECLINED, _) => StopReason::PermissionDenied,
                (code, message) => StopReason::Failed { code, message },
            };
            calling(shared, || (shared.on_stop)(reason));
        }
    }

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(
            &self,
            _stream: &SCStream,
            buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            let shared = self.ivars();
            if kind != SCStreamOutputType::Screen
                || shared.paused.load(Ordering::Acquire)
                || shared.ended.load(Ordering::Acquire)
                || unsafe { status(buffer) } != Some(COMPLETE)
            {
                return;
            }
            let Some(pixels) = (unsafe { buffer.image_buffer() }) else {
                return;
            };
            let time = unsafe { buffer.presentation_time_stamp().seconds() };
            let frame = Frame {
                buffer: pixels,
                scale: shared.scale,
                time: Duration::try_from_secs_f64(time).unwrap_or_default(),
            };
            calling(shared, || (shared.on_frame)(frame));
        }
    }
);

impl Output {
    fn new(shared: Arc<Shared>) -> Retained<Output> {
        let this = Output::alloc().set_ivars(shared);
        unsafe { msg_send![super(this), init] }
    }
}

/// The frame status ScreenCaptureKit attaches to a sample buffer.
unsafe fn status(buffer: &CMSampleBuffer) -> Option<isize> {
    let attachments = unsafe { buffer.sample_attachments_array(false) }?;
    // SAFETY: CFArray and CFDictionary are toll-free bridged to their Foundation counterparts,
    // and the attachments map strings to numbers.
    let attachments: &NSArray<NSDictionary<NSString, NSNumber>> = unsafe {
        &*(&*attachments as *const _ as *const NSArray<NSDictionary<NSString, NSNumber>>)
    };
    let first = attachments.firstObject()?;
    Some(
        first
            .objectForKey(unsafe { SCStreamFrameInfoStatus })?
            .integerValue(),
    )
}

/// The shareable content, which a completion handler hands over from ScreenCaptureKit's queue.
struct Content(Retained<SCShareableContent>);

// SAFETY: shareable content is an immutable snapshot.
unsafe impl Send for Content {}

fn shareable_content() -> Result<Content, CaptureError> {
    let (send, receive) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = match (unsafe { content.as_ref() }, unsafe { error.as_ref() }) {
                (Some(content), _) => Ok(Content(content.retain())),
                (None, Some(error)) => Err(failed(error)),
                (None, None) => Err(CaptureError::NoAnswer),
            };
            let _ = send.send(result);
        },
    );
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &handler,
        )
    };
    receive
        .recv_timeout(ANSWER)
        .map_err(|_| CaptureError::NoAnswer)?
}

/// A filter for `display` with only the windows of the helper process `helper`. A filter that
/// includes an application includes every process of it, so the other helper processes'
/// windows, which belong to other sessions, are excepted; their numbers come back too.
fn filter(
    content: &SCShareableContent,
    display: u32,
    helper: u32,
) -> Result<(Retained<SCContentFilter>, Vec<u32>, Rect), CaptureError> {
    let displays = unsafe { content.displays() };
    let display = displays
        .iter()
        .find(|candidate| unsafe { candidate.displayID() } == display)
        .ok_or(CaptureError::NoDisplay)?;
    let frame = unsafe { display.frame() };
    let bounds = Rect::new(0.0, 0.0, frame.size.width, frame.size.height);
    let applications = unsafe { content.applications() };
    let application = applications
        .iter()
        .find(|application| unsafe { application.processID() } as u32 == helper)
        .ok_or(CaptureError::NoHelper)?;
    let bundle = unsafe { application.bundleIdentifier() };
    let others: Vec<Retained<SCWindow>> = unsafe { content.windows() }
        .iter()
        .filter(|window| {
            unsafe { window.owningApplication() }.is_some_and(|owner| unsafe {
                owner.processID() as u32 != helper && owner.bundleIdentifier() == bundle
            })
        })
        .collect();
    let excepted = others
        .iter()
        .map(|window| unsafe { window.windowID() })
        .collect();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
            SCContentFilter::alloc(),
            &display,
            &NSArray::from_retained_slice(&[application]),
            &NSArray::from_retained_slice(&others),
        )
    };
    Ok((filter, excepted, bounds))
}

/// What the refresher needs of a running capture.
struct Watched {
    stream: Retained<SCStream>,
    display: u32,
    helper: u32,
    /// The other helpers' windows the filter leaves out.
    excepted: Mutex<Vec<u32>>,
}

// SAFETY: ScreenCaptureKit's stream is used from any thread.
unsafe impl Send for Watched {}
unsafe impl Sync for Watched {}

static WATCHED: Mutex<Vec<Weak<Watched>>> = Mutex::new(Vec::new());

/// Keeps leaving out the windows other helpers open, such as a neighbouring session's menu that
/// reaches into the region, for every capture of the process, with one look at the shareable
/// content each time.
fn watch(watched: &Arc<Watched>) {
    WATCHED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Arc::downgrade(watched));
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(REFRESH);
                refresh();
            }
        });
    });
}

fn refresh() {
    let live: Vec<Arc<Watched>> = {
        let mut watched = WATCHED.lock().unwrap_or_else(PoisonError::into_inner);
        watched.retain(|watched| watched.strong_count() > 0);
        watched.iter().filter_map(Weak::upgrade).collect()
    };
    if live.is_empty() {
        return;
    }
    let Ok(Content(content)) = shareable_content() else {
        return;
    };
    for watched in live {
        let Ok((filter, excepted, _)) = filter(&content, watched.display, watched.helper) else {
            continue;
        };
        let mut current = watched
            .excepted
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *current != excepted {
            unsafe {
                watched
                    .stream
                    .updateContentFilter_completionHandler(&filter, None)
            };
            *current = excepted;
        }
    }
}

/// Waits for a completion handler's error, if any.
fn completed(
    start: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), CaptureError> {
    let (send, receive) = mpsc::channel();
    let handler = RcBlock::new(move |error: *mut NSError| {
        let _ = send.send(unsafe { error.as_ref() }.map(failed));
    });
    start(&handler);
    match receive.recv_timeout(ANSWER) {
        Ok(None) => Ok(()),
        Ok(Some(error)) => Err(error),
        Err(_) => Err(CaptureError::NoAnswer),
    }
}

fn configuration(
    rect: &Rect,
    scale: f64,
    options: &CaptureOptions,
) -> Retained<SCStreamConfiguration> {
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setSourceRect(CGRect::new(
            CGPoint::new(rect.x, rect.y),
            CGSize::new(rect.width, rect.height),
        ));
        config.setWidth((rect.width * scale).round() as usize);
        config.setHeight((rect.height * scale).round() as usize);
        config.setShowsCursor(false);
        config.setPixelFormat(options.pixel_format.code());
        config.setQueueDepth(options.queue_depth as isize);
        config.setMinimumFrameInterval(CMTime::with_seconds(1.0 / options.max_fps, 600));
    }
    config
}

/// A running capture of a region with only one helper's windows. Dropping it stops the stream
/// and waits for it: no callback runs once the drop returns.
pub struct Capture {
    /// Keeps the refresher leaving out other helpers' windows while the capture runs.
    _watched: Arc<Watched>,
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    queue: DispatchRetained<DispatchQueue>,
    shared: Arc<Shared>,
    display: u32,
    /// The display's size in points.
    bounds: Rect,
    options: CaptureOptions,
}

// SAFETY: ScreenCaptureKit's stream is used from any thread; the callbacks run on the
// capture's own queue.
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

impl fmt::Debug for Capture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Capture")
            .field("display", &self.display)
            .field("scale", &self.shared.scale)
            .finish_non_exhaustive()
    }
}

impl Capture {
    /// Starts capturing `region` with only the windows of the helper process `helper_pid`
    /// (`plughost::HelperMonitor::process_id`), and returns once frames can come. Frames go to
    /// `on_frame` and a stop ScreenCaptureKit decides to `on_stop`, on the capture's own queue,
    /// one at a time; they should not block. A callback may drop the capture, after which no
    /// other reaches the application. Start it after the helper's remote editor opened:
    /// a helper without windows may not be capturable yet.
    pub fn start(
        region: &Region,
        helper_pid: u32,
        options: &CaptureOptions,
        on_frame: impl Fn(Frame) + Send + Sync + 'static,
        on_stop: impl Fn(StopReason) + Send + Sync + 'static,
    ) -> Result<Capture, CaptureError> {
        if !region.is_well_formed() {
            return Err(CaptureError::Region);
        }
        let Content(content) = shareable_content()?;
        let (filter, excepted, bounds) = filter(&content, region.display, helper_pid)?;
        if !bounds.contains(&region.rect) {
            return Err(CaptureError::Region);
        }
        let scale = f64::from(unsafe { filter.pointPixelScale() });
        let shared = Arc::new(Shared {
            on_frame: Box::new(on_frame),
            on_stop: Box::new(on_stop),
            scale,
            paused: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        });
        let output = Output::new(Arc::clone(&shared));
        let config = configuration(&region.rect, scale, options);
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        let queue = DispatchQueue::new("plughost.capture", None);
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }
        .map_err(|error| failed(&error))?;
        completed(|handler| unsafe { stream.startCaptureWithCompletionHandler(Some(handler)) })?;
        let watched = Arc::new(Watched {
            stream: stream.clone(),
            display: region.display,
            helper: helper_pid,
            excepted: Mutex::new(excepted),
        });
        watch(&watched);
        Ok(Capture {
            _watched: watched,
            stream,
            _output: output,
            queue,
            shared,
            display: region.display,
            bounds,
            options: *options,
        })
    }

    /// Captures `region` instead, on the same display; it moves with the application's layout.
    pub fn set_region(&self, region: &Region) -> Result<(), CaptureError> {
        if !region.is_well_formed()
            || region.display != self.display
            || !self.bounds.contains(&region.rect)
        {
            return Err(CaptureError::Region);
        }
        let config = configuration(&region.rect, self.shared.scale, &self.options);
        completed(|handler| unsafe {
            self.stream
                .updateConfiguration_completionHandler(&config, Some(handler))
        })
    }

    /// Stops handing frames to the application while no one watches, without stopping the
    /// stream. Frames come only when the region changes, so the first frame after
    /// [`Capture::resume`] waits for a change; keep the last one to show until then.
    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Release);
    }

    /// Hands frames to the application again.
    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Release);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shared.ended.store(true, Ordering::Release);
        let _ = completed(|handler| unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(handler))
        });
        // A callback that began before the stop finishes before the drop returns, unless the
        // drop is in that callback.
        if !std::ptr::eq(CALLING.get(), Arc::as_ptr(&self.shared)) {
            self.queue.exec_sync(|| {});
        }
    }
}
