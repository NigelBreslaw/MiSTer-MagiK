//! Portable controller state and layout-profile naming.

pub use crate::input_info::PadInfo;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadRawEvent {
    pub event_type: u8,
    pub number: u8,
    pub value: i16,
}

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
    pub last_raw_event: Option<PadRawEvent>,
    pub last_raw: String,
    /// Human-readable list of everything currently held.
    pub pressed_now: String,
    /// Friendly label for the most recent event (for mapping debug).
    pub last_event_label: String,
}

impl PadState {
    pub fn record_raw_event(&mut self, event_type: u8, number: u8, value: i16, debug_labels: bool) {
        self.last_raw_event = Some(PadRawEvent {
            event_type,
            number,
            value,
        });
        if debug_labels {
            self.last_raw = format!("type={event_type} num={number} val={value}");
        } else {
            self.last_raw.clear();
            self.last_event_label.clear();
        }
    }

    pub fn set_debug_event_label(&mut self, debug_labels: bool, label: impl FnOnce() -> String) {
        if debug_labels {
            self.last_event_label = label();
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_event_without_debug_labels_keeps_compact_data_only() {
        let mut state = PadState {
            last_raw: "old raw".to_string(),
            last_event_label: "old label".to_string(),
            ..PadState::default()
        };

        state.record_raw_event(1, 2, 3, false);

        assert_eq!(
            state.last_raw_event,
            Some(PadRawEvent {
                event_type: 1,
                number: 2,
                value: 3
            })
        );
        assert!(state.last_raw.is_empty());
        assert!(state.last_event_label.is_empty());
    }

    #[test]
    fn raw_event_with_debug_labels_formats_strings() {
        let mut state = PadState::default();

        state.record_raw_event(1, 2, 3, true);
        state.set_debug_event_label(true, || "A down".to_string());

        assert_eq!(state.last_raw, "type=1 num=2 val=3");
        assert_eq!(state.last_event_label, "A down");
    }
}
