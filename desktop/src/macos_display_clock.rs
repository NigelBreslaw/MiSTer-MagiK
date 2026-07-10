use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSView;
use objc2_foundation::{NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::CADisplayLink;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MacDisplayLinkTick {
    pub timestamp_us: u64,
    pub target_timestamp_us: u64,
    pub duration_us: u64,
}

pub type MacDisplayLinkCallback = Rc<RefCell<Box<dyn FnMut(MacDisplayLinkTick)>>>;

struct DisplayLinkTargetIvars {
    callback: MacDisplayLinkCallback,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DisplayLinkTargetIvars]
    struct DisplayLinkTarget;

    unsafe impl NSObjectProtocol for DisplayLinkTarget {}

    impl DisplayLinkTarget {
        #[unsafe(method(tick:))]
        fn tick(&self, display_link: &CADisplayLink) {
            (self.ivars().callback.borrow_mut())(MacDisplayLinkTick {
                timestamp_us: seconds_to_us(display_link.timestamp()),
                target_timestamp_us: seconds_to_us(display_link.targetTimestamp()),
                duration_us: seconds_to_us(display_link.duration()),
            });
        }
    }
);

impl DisplayLinkTarget {
    fn new(mtm: MainThreadMarker, callback: MacDisplayLinkCallback) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DisplayLinkTargetIvars { callback });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct MacDisplayClock {
    _target: Retained<DisplayLinkTarget>,
    display_link: Retained<CADisplayLink>,
}

impl MacDisplayClock {
    pub fn start(
        window: &slint::winit_030::winit::window::Window,
        callback: MacDisplayLinkCallback,
    ) -> Option<Self> {
        use objc2::runtime::AnyClass;
        use objc2::sel;

        if !AnyClass::get(c"NSView")?.responds_to(sel!(displayLinkWithTarget:selector:)) {
            return None;
        }
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: The AppKit view is owned by this live winit window and this
        // function is called from Slint's macOS UI event loop.
        let ns_view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
        let mtm = MainThreadMarker::new()?;
        let target = DisplayLinkTarget::new(mtm, callback);
        // SAFETY: DisplayLinkTarget implements the tick: selector with the
        // CADisplayLink argument required by NSView.
        let display_link = unsafe { ns_view.displayLinkWithTarget_selector(&target, sel!(tick:)) };
        // SAFETY: The main run loop and common mode are used on the main thread.
        unsafe {
            display_link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
        }
        display_link.setPaused(false);
        Some(Self {
            _target: target,
            display_link,
        })
    }
}

impl Drop for MacDisplayClock {
    fn drop(&mut self) {
        self.display_link.invalidate();
    }
}

fn seconds_to_us(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1_000_000.0).round() as u64
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::seconds_to_us;

    #[test]
    fn display_link_seconds_convert_to_microseconds() {
        assert_eq!(seconds_to_us(1.25), 1_250_000);
        assert_eq!(seconds_to_us(1.0 / 120.0), 8_333);
        assert_eq!(seconds_to_us(f64::NAN), 0);
        assert_eq!(seconds_to_us(-1.0), 0);
    }
}
