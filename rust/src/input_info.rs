//! Controller identity metadata shared by host-testable logic and Linux input.

/// Static metadata read from sysfs / ioctl when the pad is opened.
#[derive(Debug, Clone, Default)]
pub struct PadInfo {
    pub name: String,
    pub vendor_id: String,
    pub product_id: String,
    pub serial: String,
    pub phys: String,
    /// USB topology port, e.g. `1-1.3` from sysfs path / `usb-...-1.3` from phys.
    pub usb_port: String,
    pub js_buttons: u8,
    pub js_axes: u8,
    pub evdev_key_count: usize,
    pub evdev_abs_count: usize,
    /// False when the kernel js API exposes no button slot beyond Home (js 12).
    pub capture_available: bool,
}
