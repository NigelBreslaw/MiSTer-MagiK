// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable controller state and layout-profile naming.

use crate::input_event::LogicalAction;
pub use crate::input_info::PadInfo;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadRawEvent {
    pub event_type: u8,
    pub number: u8,
    pub value: i16,
}

pub const JS_EVENT_BUTTON: u8 = 0x01;
pub const JS_EVENT_AXIS: u8 = 0x02;
const AXIS_MAX: f32 = 32767.0;
const STICK_DEADZONE: f32 = 8000.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectionalState {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

impl DirectionalState {
    #[must_use]
    pub const fn from_pad(state: &PadState) -> Self {
        Self {
            up: state.dpad_up && !state.dpad_down,
            down: state.dpad_down && !state.dpad_up,
            left: state.dpad_left && !state.dpad_right,
            right: state.dpad_right && !state.dpad_left,
        }
    }

    #[must_use]
    pub const fn is_neutral(self) -> bool {
        !self.up && !self.down && !self.left && !self.right
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DirectionalEdges {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

impl DirectionalEdges {
    #[must_use]
    pub const fn rising(current: DirectionalState, previous: DirectionalState) -> Self {
        Self {
            up: current.up && !previous.up,
            down: current.down && !previous.down,
            left: current.left && !previous.left,
            right: current.right && !previous.right,
        }
    }
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
    #[doc(hidden)]
    pub generic_direction_axes: [i16; 8],
    pub last_raw_event: Option<PadRawEvent>,
    pub last_raw: String,
    /// Human-readable list of everything currently held.
    pub pressed_now: String,
    /// Friendly label for the most recent event (for mapping debug).
    pub last_event_label: String,
}

impl PadState {
    pub fn set_logical_action(&mut self, action: LogicalAction, held: bool) {
        match action {
            LogicalAction::Up => self.dpad_up = held,
            LogicalAction::Down => self.dpad_down = held,
            LogicalAction::Left => self.dpad_left = held,
            LogicalAction::Right => self.dpad_right = held,
            LogicalAction::Activate => self.btn_a = held,
            LogicalAction::Back => self.btn_b = held,
            LogicalAction::Home => self.btn_home = held,
            LogicalAction::X => self.btn_x = held,
            LogicalAction::Y => self.btn_y = held,
            LogicalAction::L => self.btn_l = held,
            LogicalAction::R => self.btn_r = held,
            LogicalAction::Select => self.btn_select = held,
            LogicalAction::Start => self.btn_start = held,
        }
        self.rebuild_pressed_now();
    }

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

    pub fn rebuild_pressed_now(&mut self) {
        let mut parts: Vec<&str> = Vec::new();
        if self.dpad_up {
            parts.push("D-Up");
        }
        if self.dpad_down {
            parts.push("D-Down");
        }
        if self.dpad_left {
            parts.push("D-Left");
        }
        if self.dpad_right {
            parts.push("D-Right");
        }
        macro_rules! btn {
            ($field:ident, $name:expr) => {
                if self.$field {
                    parts.push($name);
                }
            };
        }
        btn!(btn_y, "Y");
        btn!(btn_b, "B");
        btn!(btn_a, "A");
        btn!(btn_x, "X");
        btn!(btn_l, "L");
        btn!(btn_r, "R");
        btn!(btn_zl, "ZL");
        btn!(btn_zr, "ZR");
        btn!(btn_select, "Select");
        btn!(btn_start, "Start");
        btn!(btn_l3, "L3");
        btn!(btn_r3, "R3");
        btn!(btn_home, "Home");
        btn!(btn_capture, "Capture");
        if self.left_x.abs() > 0.01 || self.left_y.abs() > 0.01 {
            parts.push("Left stick");
        }
        if self.right_x.abs() > 0.01 || self.right_y.abs() > 0.01 {
            parts.push("Right stick");
        }
        self.pressed_now = if parts.is_empty() {
            "—".into()
        } else {
            parts.join(", ")
        };
    }

    fn apply_dpad_x(&mut self, v: f32, label: &'static str) -> &'static str {
        if v < -STICK_DEADZONE {
            self.dpad_left = true;
            self.dpad_right = false;
        } else if v > STICK_DEADZONE {
            self.dpad_right = true;
            self.dpad_left = false;
        } else {
            self.dpad_left = false;
            self.dpad_right = false;
        }
        label
    }

    fn apply_dpad_y(&mut self, v: f32, label: &'static str) -> &'static str {
        if v < -STICK_DEADZONE {
            self.dpad_up = true;
            self.dpad_down = false;
        } else if v > STICK_DEADZONE {
            self.dpad_down = true;
            self.dpad_up = false;
        } else {
            self.dpad_up = false;
            self.dpad_down = false;
        }
        label
    }

    fn record_generic_direction_axis(&mut self, axis: u8, value: i16) {
        self.generic_direction_axes[axis as usize] = value;
        let held = |axis: usize, positive: bool| {
            let value = self.generic_direction_axes[axis];
            if axis >= 6 {
                if positive { value > 0 } else { value < 0 }
            } else if positive {
                value as f32 > STICK_DEADZONE
            } else {
                (value as f32) < -STICK_DEADZONE
            }
        };
        self.dpad_left = [0, 4, 6].into_iter().any(|axis| held(axis, false));
        self.dpad_right = [0, 4, 6].into_iter().any(|axis| held(axis, true));
        self.dpad_up = [1, 5, 7].into_iter().any(|axis| held(axis, false));
        self.dpad_down = [1, 5, 7].into_iter().any(|axis| held(axis, true));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputProfile {
    layout: PadLayout,
}

impl InputProfile {
    pub fn guess(info: &PadInfo) -> Self {
        Self {
            layout: PadLayout::guess(info),
        }
    }

    pub fn generic() -> Self {
        Self {
            layout: PadLayout::Generic,
        }
    }

    pub fn dpad_axes_45() -> Self {
        Self {
            layout: PadLayout::DpadAxes45,
        }
    }

    pub fn name(self) -> &'static str {
        self.layout.profile_name()
    }

    pub fn apply_js_event(
        self,
        state: &mut PadState,
        event: PadRawEvent,
        debug_labels: bool,
    ) -> bool {
        state.record_raw_event(event.event_type, event.number, event.value, debug_labels);
        match self.layout {
            PadLayout::DpadAxes45 => self.apply_event_dpad_axes_45(state, event, debug_labels),
            PadLayout::Generic => self.apply_event_generic(state, event, debug_labels),
        }
    }

    fn apply_event_dpad_axes_45(
        self,
        state: &mut PadState,
        event: PadRawEvent,
        debug_labels: bool,
    ) -> bool {
        let changed = match event.event_type {
            JS_EVENT_BUTTON => {
                let pressed = event.value != 0;
                let label = match event.number {
                    0 => {
                        state.btn_y = pressed;
                        "Y"
                    }
                    1 => {
                        state.btn_b = pressed;
                        "B"
                    }
                    2 => {
                        state.btn_a = pressed;
                        "A"
                    }
                    3 => {
                        state.btn_x = pressed;
                        "X"
                    }
                    4 => {
                        state.btn_l = pressed;
                        "L"
                    }
                    5 => {
                        state.btn_r = pressed;
                        "R"
                    }
                    6 => {
                        state.btn_zl = pressed;
                        "ZL"
                    }
                    7 => {
                        state.btn_zr = pressed;
                        "ZR"
                    }
                    8 => {
                        state.btn_select = pressed;
                        "Select"
                    }
                    9 => {
                        state.btn_start = pressed;
                        "Start"
                    }
                    10 => {
                        state.btn_l3 = pressed;
                        "L3"
                    }
                    11 => {
                        state.btn_r3 = pressed;
                        "R3"
                    }
                    12 => {
                        state.btn_home = pressed;
                        "Home"
                    }
                    13 => {
                        state.btn_capture = pressed;
                        "Capture"
                    }
                    _ => {
                        state.set_debug_event_label(debug_labels, || {
                            format!(
                                "unknown btn {} {}",
                                event.number,
                                if pressed { "down" } else { "up" }
                            )
                        });
                        state.rebuild_pressed_now();
                        return false;
                    }
                };
                state.set_debug_event_label(debug_labels, || {
                    format!(
                        "{label} {} (js btn {})",
                        if pressed { "down" } else { "up" },
                        event.number
                    )
                });
                true
            }
            JS_EVENT_AXIS => {
                let v = event.value as f32;
                let label = match event.number {
                    0 => {
                        state.left_x = normalize_stick(v);
                        "Left X"
                    }
                    1 => {
                        state.left_y = normalize_stick(v);
                        "Left Y"
                    }
                    2 => {
                        state.right_x = normalize_stick(v);
                        "Right X"
                    }
                    3 => {
                        state.right_y = normalize_stick(v);
                        "Right Y"
                    }
                    4 => state.apply_dpad_x(v, "D-pad X"),
                    5 => state.apply_dpad_y(v, "D-pad Y"),
                    _ => {
                        state.set_debug_event_label(debug_labels, || {
                            format!("unknown axis {} val={}", event.number, event.value)
                        });
                        state.rebuild_pressed_now();
                        return false;
                    }
                };
                if event.number <= 3 {
                    state.set_debug_event_label(debug_labels, || {
                        format!("{label} axis {} = {}", event.number, event.value)
                    });
                }
                true
            }
            _ => return false,
        };
        state.rebuild_pressed_now();
        changed
    }

    fn apply_event_generic(
        self,
        state: &mut PadState,
        event: PadRawEvent,
        debug_labels: bool,
    ) -> bool {
        let changed = match event.event_type {
            JS_EVENT_BUTTON => {
                let pressed = event.value != 0;
                let label = match event.number {
                    0 => {
                        state.btn_a = pressed;
                        "A"
                    }
                    1 => {
                        state.btn_b = pressed;
                        "B"
                    }
                    2 => {
                        state.btn_x = pressed;
                        "X"
                    }
                    3 => {
                        state.btn_y = pressed;
                        "Y"
                    }
                    4 => {
                        state.btn_l = pressed;
                        "L"
                    }
                    5 => {
                        state.btn_r = pressed;
                        "R"
                    }
                    6 => {
                        state.btn_select = pressed;
                        "Select"
                    }
                    7 => {
                        state.btn_start = pressed;
                        "Start"
                    }
                    8 => {
                        state.btn_l3 = pressed;
                        "L3"
                    }
                    9 => {
                        state.btn_r3 = pressed;
                        "R3"
                    }
                    10 | 11 => {
                        state.btn_home = pressed;
                        "Home"
                    }
                    13 => {
                        state.btn_capture = pressed;
                        "Capture"
                    }
                    _ => {
                        state.set_debug_event_label(debug_labels, || {
                            format!(
                                "unknown btn {} {}",
                                event.number,
                                if pressed { "down" } else { "up" }
                            )
                        });
                        state.rebuild_pressed_now();
                        return false;
                    }
                };
                state.set_debug_event_label(debug_labels, || {
                    format!(
                        "{label} {} (js btn {})",
                        if pressed { "down" } else { "up" },
                        event.number
                    )
                });
                true
            }
            JS_EVENT_AXIS => {
                let v = event.value as f32;
                match event.number {
                    0 => {
                        state.left_x = normalize_stick(v);
                        state.record_generic_direction_axis(event.number, event.value);
                    }
                    1 => {
                        state.left_y = normalize_stick(v);
                        state.record_generic_direction_axis(event.number, event.value);
                    }
                    2 => {
                        state.right_x = normalize_stick(v);
                    }
                    3 => {
                        state.right_y = normalize_stick(v);
                    }
                    4 => {
                        state.record_generic_direction_axis(event.number, event.value);
                    }
                    5 => {
                        state.record_generic_direction_axis(event.number, event.value);
                    }
                    6 => {
                        state.record_generic_direction_axis(event.number, event.value);
                    }
                    7 => {
                        state.record_generic_direction_axis(event.number, event.value);
                    }
                    _ => {
                        state.set_debug_event_label(debug_labels, || {
                            format!("unknown axis {} val={}", event.number, event.value)
                        });
                        state.rebuild_pressed_now();
                        return false;
                    }
                };
                state.set_debug_event_label(debug_labels, || {
                    format!("axis {} = {}", event.number, event.value)
                });
                true
            }
            _ => return false,
        };
        state.rebuild_pressed_now();
        changed
    }
}

/// Name of the input profile used to decode js events for this pad.
pub fn layout_profile_name(info: &PadInfo) -> &'static str {
    InputProfile::guess(info).name()
}

fn strip_hex_prefix(raw: &str) -> String {
    raw.trim()
        .strip_prefix("0x")
        .or_else(|| raw.trim().strip_prefix("0X"))
        .unwrap_or(raw.trim())
        .to_ascii_lowercase()
}

fn normalize_stick(v: f32) -> f32 {
    if v.abs() < STICK_DEADZONE {
        return 0.0;
    }
    (v / AXIS_MAX).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directional_edges_cancel_opposites_and_fire_once() {
        let pad = PadState {
            dpad_left: true,
            dpad_right: true,
            dpad_up: true,
            ..PadState::default()
        };
        let current = DirectionalState::from_pad(&pad);
        assert!(!current.left);
        assert!(!current.right);
        assert!(current.up);
        let first = DirectionalEdges::rising(current, DirectionalState::default());
        assert!(first.up);
        assert!(!DirectionalEdges::rising(current, current).up);
    }

    #[test]
    fn logical_actions_update_only_their_normalized_control() {
        let mut state = PadState::default();

        state.set_logical_action(LogicalAction::Activate, true);
        state.set_logical_action(LogicalAction::Left, true);
        assert!(state.btn_a);
        assert!(state.dpad_left);
        assert_eq!(state.pressed_now, "D-Left+A");

        state.set_logical_action(LogicalAction::Activate, false);
        assert!(!state.btn_a);
        assert!(state.dpad_left);
        assert_eq!(state.pressed_now, "D-Left");
    }

    fn js_button(number: u8, value: i16) -> PadRawEvent {
        PadRawEvent {
            event_type: JS_EVENT_BUTTON,
            number,
            value,
        }
    }

    fn js_axis(number: u8, value: i16) -> PadRawEvent {
        PadRawEvent {
            event_type: JS_EVENT_AXIS,
            number,
            value,
        }
    }

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

    #[test]
    fn generic_button_zero_maps_to_a_without_debug_strings() {
        let mut state = PadState::default();

        assert!(InputProfile::generic().apply_js_event(&mut state, js_button(0, 1), false));

        assert!(state.btn_a);
        assert_eq!(state.last_raw_event, Some(js_button(0, 1)));
        assert!(state.last_raw.is_empty());
        assert!(state.last_event_label.is_empty());
        assert_eq!(state.pressed_now, "A");
    }

    #[test]
    fn generic_button_13_maps_to_capture() {
        let mut state = PadState::default();

        assert!(InputProfile::generic().apply_js_event(&mut state, js_button(13, 1), true));

        assert!(state.btn_capture);
        assert_eq!(state.last_event_label, "Capture down (js btn 13)");
        assert_eq!(state.pressed_now, "Capture");
    }

    #[test]
    fn generic_axis_zero_updates_left_stick_and_stick_dpad() {
        let mut state = PadState::default();

        assert!(InputProfile::generic().apply_js_event(&mut state, js_axis(0, -32767), true));

        assert!(state.dpad_left);
        assert!(!state.dpad_right);
        assert_eq!(state.left_x, -1.0);
        assert_eq!(state.last_event_label, "axis 0 = -32767");
        assert!(state.pressed_now.contains("D-Left"));
        assert!(state.pressed_now.contains("Left stick"));
    }

    #[test]
    fn generic_direction_sources_do_not_release_each_other() {
        let mut state = PadState::default();
        let profile = InputProfile::generic();

        profile.apply_js_event(&mut state, js_axis(0, 32767), false);
        profile.apply_js_event(&mut state, js_axis(4, 32767), false);
        profile.apply_js_event(&mut state, js_axis(4, 0), false);
        assert!(state.dpad_right);

        profile.apply_js_event(&mut state, js_axis(6, -32767), false);
        assert!(state.dpad_left);
        assert!(state.dpad_right);

        profile.apply_js_event(&mut state, js_axis(0, 0), false);
        assert!(!state.dpad_right);
        assert!(state.dpad_left);
    }

    #[test]
    fn dpad_axes_45_maps_js0_to_y_js2_to_a() {
        let mut state = PadState::default();
        let profile = InputProfile::dpad_axes_45();

        assert!(profile.apply_js_event(&mut state, js_button(0, 1), true));
        assert!(profile.apply_js_event(&mut state, js_button(2, 1), true));

        assert!(state.btn_y);
        assert!(state.btn_a);
        assert!(!state.btn_b);
        assert_eq!(state.last_event_label, "A down (js btn 2)");
        assert_eq!(state.pressed_now, "Y, A");
    }

    #[test]
    fn dpad_axes_45_axis_4_5_drive_dpad() {
        let mut state = PadState::default();
        let profile = InputProfile::dpad_axes_45();

        assert!(profile.apply_js_event(&mut state, js_axis(4, 32767), true));
        assert!(profile.apply_js_event(&mut state, js_axis(5, -32767), true));

        assert!(state.dpad_right);
        assert!(!state.dpad_left);
        assert!(state.dpad_up);
        assert!(!state.dpad_down);
        assert_eq!(state.last_raw, "type=2 num=5 val=-32767");
        assert!(state.last_event_label.is_empty());
        assert_eq!(state.pressed_now, "D-Up, D-Right");
    }

    #[test]
    fn unknown_button_records_raw_event_but_returns_false() {
        let mut state = PadState::default();

        assert!(!InputProfile::generic().apply_js_event(&mut state, js_button(99, 1), true));

        assert_eq!(state.last_raw_event, Some(js_button(99, 1)));
        assert_eq!(state.last_raw, "type=1 num=99 val=1");
        assert_eq!(state.last_event_label, "unknown btn 99 down");
        assert_eq!(state.pressed_now, "—");
    }

    #[test]
    fn layout_guess_strips_hex_prefixes_and_identifies_known_dpad_axis_pads() {
        let retrobit = PadInfo {
            vendor_id: "0x2563".to_string(),
            product_id: "0X0575".to_string(),
            ..PadInfo::default()
        };
        let dragonrise = PadInfo {
            vendor_id: "0079".to_string(),
            product_id: "0011".to_string(),
            ..PadInfo::default()
        };
        let generic = PadInfo {
            vendor_id: "045e".to_string(),
            product_id: "028e".to_string(),
            ..PadInfo::default()
        };

        assert_eq!(InputProfile::guess(&retrobit), InputProfile::dpad_axes_45());
        assert_eq!(layout_profile_name(&retrobit), "dpad_axes_4_5");
        assert_eq!(
            InputProfile::guess(&dragonrise),
            InputProfile::dpad_axes_45()
        );
        assert_eq!(layout_profile_name(&generic), "generic");
    }

    #[test]
    fn extreme_axes_are_clamped_and_deadzone_release_clears_direction() {
        let mut state = PadState::default();
        let profile = InputProfile::generic();

        assert!(profile.apply_js_event(&mut state, js_axis(0, i16::MIN), false));
        assert_eq!(state.left_x, -1.0);
        assert!(state.dpad_left);

        assert!(profile.apply_js_event(&mut state, js_axis(0, 0), false));
        assert_eq!(state.left_x, 0.0);
        assert!(!state.dpad_left);
        assert!(!state.dpad_right);
        assert_eq!(state.pressed_now, "—");
    }

    #[test]
    fn repeated_button_events_are_idempotent_and_release_clears_summary() {
        let mut state = PadState::default();
        let profile = InputProfile::generic();

        assert!(profile.apply_js_event(&mut state, js_button(0, 1), false));
        assert!(profile.apply_js_event(&mut state, js_button(0, 1), false));
        assert_eq!(state.pressed_now, "A");

        assert!(profile.apply_js_event(&mut state, js_button(0, 0), false));
        assert!(!state.btn_a);
        assert_eq!(state.pressed_now, "—");
    }

    #[test]
    fn unknown_event_type_preserves_held_state_and_records_diagnostics() {
        let mut state = PadState::default();
        let profile = InputProfile::generic();
        profile.apply_js_event(&mut state, js_button(0, 1), false);

        let unknown = PadRawEvent {
            event_type: 0xff,
            number: 7,
            value: -2,
        };
        assert!(!profile.apply_js_event(&mut state, unknown, true));
        assert!(state.btn_a);
        assert_eq!(state.last_raw_event, Some(unknown));
        assert_eq!(state.pressed_now, "A");
    }

    #[test]
    fn generic_profile_maps_every_supported_button_and_axis() {
        let profile = InputProfile::generic();
        let mut state = PadState::default();

        for button in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13] {
            assert!(profile.apply_js_event(&mut state, js_button(button, 1), true));
        }
        assert_eq!(
            state.pressed_now,
            "Y, B, A, X, L, R, Select, Start, L3, R3, Home, Capture"
        );

        for axis in 0..=7 {
            assert!(profile.apply_js_event(&mut state, js_axis(axis, 32767), true));
        }
        assert!(state.dpad_right);
        assert!(state.dpad_down);
        assert_eq!(state.left_x, 1.0);
        assert_eq!(state.left_y, 1.0);
        assert_eq!(state.right_x, 1.0);
        assert_eq!(state.right_y, 1.0);

        assert!(!profile.apply_js_event(&mut state, js_axis(8, 1), true));
        assert_eq!(state.last_event_label, "unknown axis 8 val=1");
    }

    #[test]
    fn dpad_axes_profile_maps_every_supported_button_and_axis() {
        let profile = InputProfile::dpad_axes_45();
        let mut state = PadState::default();

        for button in 0..=13 {
            assert!(profile.apply_js_event(&mut state, js_button(button, 1), true));
        }

        for axis in 0..=5 {
            assert!(profile.apply_js_event(&mut state, js_axis(axis, -32767), true));
        }
        assert!(state.dpad_left);
        assert!(state.dpad_up);
        assert_eq!(state.left_x, -1.0);
        assert_eq!(state.left_y, -1.0);
        assert_eq!(state.right_x, -1.0);
        assert_eq!(state.right_y, -1.0);
        assert_eq!(
            state.pressed_now,
            "D-Up, D-Left, Y, B, A, X, L, R, ZL, ZR, Select, Start, L3, R3, Home, Capture, Left stick, Right stick"
        );

        assert!(!profile.apply_js_event(&mut state, js_axis(6, -1), true));
        assert_eq!(state.last_event_label, "unknown axis 6 val=-1");
    }

    #[test]
    fn pressed_summary_orders_dpad_buttons_and_sticks() {
        let mut state = PadState {
            dpad_up: true,
            dpad_down: true,
            dpad_left: true,
            dpad_right: true,
            btn_y: true,
            btn_b: true,
            btn_a: true,
            btn_x: true,
            btn_l: true,
            btn_r: true,
            btn_zl: true,
            btn_zr: true,
            btn_select: true,
            btn_start: true,
            btn_l3: true,
            btn_r3: true,
            btn_home: true,
            btn_capture: true,
            left_x: 0.02,
            right_y: -0.02,
            ..PadState::default()
        };

        state.rebuild_pressed_now();

        assert_eq!(
            state.pressed_now,
            "D-Up, D-Down, D-Left, D-Right, Y, B, A, X, L, R, ZL, ZR, Select, Start, L3, R3, Home, Capture, Left stick, Right stick"
        );
    }
}
