//! Portable controller state and layout-profile naming.

pub use crate::input_info::PadInfo;

/// Best-guess D-Input map for Retro-bit 2563:0575 (A2 receiver).
/// D-pad on axes 4/5 confirmed on device; buttons from LEGACY16 manual.
#[derive(Debug, Clone, Default)]
pub struct PadState {
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub btn_a: bool,
    pub btn_b: bool,
    pub btn_x: bool,
    pub btn_y: bool,
    pub btn_l: bool,
    pub btn_r: bool,
    pub btn_zl: bool,
    pub btn_zr: bool,
    pub btn_select: bool,
    pub btn_start: bool,
    pub btn_l3: bool,
    pub btn_r3: bool,
    pub btn_home: bool,
    pub btn_capture: bool,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
    pub last_raw: String,
    /// Human-readable list of everything currently held.
    pub pressed_now: String,
    /// Friendly label for the most recent event (for mapping debug).
    pub last_event_label: String,
}

/// How raw js button/axis indices map onto [`PadState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadLayout {
    /// D-pad reported on axes 4/5 (common on some 2.4 GHz dongles).
    DpadAxes45,
    /// Fallback: hat 6/7, d-pad 4/5, stick-as-dpad, common btn order.
    Generic,
}

impl PadLayout {
    /// Best-effort profile until the setup wizard saves a per-controller map.
    pub fn guess(info: &PadInfo) -> Self {
        match (
            strip_hex_prefix(&info.vendor_id).as_str(),
            strip_hex_prefix(&info.product_id).as_str(),
        ) {
            ("2563", "0575") | ("0079", "0011") => Self::DpadAxes45,
            _ => Self::Generic,
        }
    }

    pub fn profile_name(self) -> &'static str {
        match self {
            Self::DpadAxes45 => "dpad_axes_4_5",
            Self::Generic => "generic",
        }
    }
}

/// Name of the input profile used to decode js events for this pad.
pub fn layout_profile_name(info: &PadInfo) -> &'static str {
    PadLayout::guess(info).profile_name()
}

fn strip_hex_prefix(raw: &str) -> String {
    raw.trim()
        .strip_prefix("0x")
        .or_else(|| raw.trim().strip_prefix("0X"))
        .unwrap_or(raw.trim())
        .to_ascii_lowercase()
}
