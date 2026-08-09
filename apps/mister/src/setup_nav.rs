// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Controller setup flow — detect unknown / moved pads and offer rebinding.

use crate::controller_db::{ControllerDb, ControllerKind, PadRegistryStatus};
use crate::input_event::DeviceInstanceId;
use crate::input_event::{InputEvent, InputPhase, LogicalAction};
#[cfg(test)]
use crate::input_repeat::RepeatNav;
use crate::input_state::{PadInfo, PadState, layout_profile_name};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupPhase {
    None = 0,
    /// "New input detected" — press any button to continue.
    Detected = 1,
    /// Moved port: new controller vs pick from registry.
    NewOrExisting = 2,
    /// Scroll list of saved controllers and confirm.
    PickExisting = 3,
    /// Show everything we know about this pad.
    Configure = 4,
    /// Name (from kernel for now) + controller type.
    NameKind = 5,
}

pub struct SetupNav {
    pub phase: SetupPhase,
    /// Registry status that triggered the current flow.
    pub trigger_status: PadRegistryStatus,
    /// Which pad in the pool this dialog refers to.
    pub target_pad_idx: usize,
    /// Stable identity used by the event-driven input path.
    pub target_device: Option<DeviceInstanceId>,
    pub list_index: usize,
    pub draft_label: String,
    pub draft_kind: ControllerKind,
    #[cfg(test)]
    test_repeat: RepeatNav,
    #[cfg(test)]
    test_prev: PadState,
    /// Ignore the triggering edge on the same frame we opened from pad activity.
    armed: bool,
}

pub enum SetupAction {
    None,
    /// User confirmed this is a new controller identity (MovedPort → "New controller").
    RegisterNew,
    /// User picked an existing registry entry by index in `list_entries()`.
    ClaimExisting {
        list_index: usize,
    },
    /// Save label + kind and mark setup complete.
    SaveFinish {
        label: String,
        kind: ControllerKind,
    },
    /// Pad already complete — close this flow (e.g. after USB rebind).
    Done,
}

pub trait SetupPadSource {
    fn index_needing_setup(&self) -> Option<usize>;
    fn db(&self) -> &ControllerDb;
    fn info_at(&self, idx: usize) -> &PadInfo;
}

impl SetupNav {
    pub fn new() -> Self {
        Self {
            phase: SetupPhase::None,
            trigger_status: PadRegistryStatus::Unknown,
            target_pad_idx: 0,
            target_device: None,
            list_index: 0,
            draft_label: String::new(),
            draft_kind: ControllerKind::Unknown,
            #[cfg(test)]
            test_repeat: RepeatNav::default(),
            #[cfg(test)]
            test_prev: PadState::default(),
            armed: false,
        }
    }

    pub fn open_for(&mut self, status: PadRegistryStatus, pad_idx: usize) {
        self.trigger_status = status;
        self.target_pad_idx = pad_idx;
        self.target_device = None;
        self.phase = SetupPhase::Detected;
        self.list_index = 0;
        // Startup / programmatic open — accept input on the first press.
        self.armed = true;
    }

    pub fn open_for_device(
        &mut self,
        status: PadRegistryStatus,
        device: DeviceInstanceId,
        current_index: usize,
    ) {
        self.open_for(status, current_index);
        self.target_device = Some(device);
    }

    pub fn is_active(&self) -> bool {
        self.phase != SetupPhase::None
    }

    /// Call after pad input when no setup dialog is showing.
    pub fn maybe_open(
        &mut self,
        info: &PadInfo,
        pad_idx: usize,
        db: &ControllerDb,
        had_activity: bool,
    ) {
        if self.phase != SetupPhase::None || !had_activity {
            return;
        }
        let status = db.registry_status(info);
        if db.needs_setup(info) {
            self.trigger_status = status;
            self.target_pad_idx = pad_idx;
            self.target_device = None;
            self.phase = SetupPhase::Detected;
            self.list_index = 0;
            // Debounce the button press that triggered detection.
            self.armed = false;
        }
    }

    pub fn title(&self) -> String {
        match self.phase {
            SetupPhase::None => String::new(),
            SetupPhase::Detected => "New input detected".into(),
            SetupPhase::NewOrExisting => "Is this a new controller?".into(),
            SetupPhase::PickExisting => "Select your controller".into(),
            SetupPhase::Configure => "Controller details".into(),
            SetupPhase::NameKind => "Name this controller".into(),
        }
    }

    pub fn subtitle(&self, info: &PadInfo, db: &ControllerDb) -> String {
        let name = db.display_label(info);
        match self.phase {
            SetupPhase::None => String::new(),
            SetupPhase::Detected => {
                if self.trigger_status == PadRegistryStatus::MovedPort {
                    format!(
                        "{name}\nPlugged in at {}\nPress any button to continue",
                        info.usb_port
                    )
                } else if self.trigger_status == PadRegistryStatus::PendingSetup {
                    format!("{name}\nSetup not finished\nPress any button to continue")
                } else {
                    format!("{name}\nPress any button to continue")
                }
            }
            SetupPhase::NewOrExisting => format!(
                "{name} at port {}\nWas this controller set up before on a different USB port?",
                info.usb_port
            ),
            SetupPhase::PickExisting => format!(
                "Choose which saved controller is plugged in at {}",
                info.usb_port
            ),
            SetupPhase::Configure => String::new(),
            SetupPhase::NameKind => {
                "Default name from the device — keyboard rename coming soon".into()
            }
        }
    }

    pub fn draft_kind_label(&self) -> &'static str {
        self.draft_kind.label()
    }

    /// After saving or skipping, open the next pad that still needs setup.
    pub fn advance_to_next_pad(&mut self, pad: &impl SetupPadSource) {
        self.phase = SetupPhase::None;
        self.target_device = None;
        if let Some(idx) = pad.index_needing_setup() {
            let status = pad.db().registry_status(pad.info_at(idx));
            crate::ui_errln!("controller setup: advancing to pad {idx} ({status:?})");
            self.open_for(status, idx);
        }
    }

    fn begin_name_kind(&mut self, info: &PadInfo, db: &ControllerDb) {
        self.draft_label = db
            .get(info)
            .map(|e| e.label.clone())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| default_draft_label(info));
        self.draft_kind = db
            .get(info)
            .map(|e| e.kind)
            .unwrap_or_else(|| ControllerDb::infer_kind(info));
        self.list_index = 0;
        self.phase = SetupPhase::NameKind;
    }

    /// Label/value rows for the configure screen.
    pub fn configure_fields(
        info: &PadInfo,
        js_path: &str,
        db: &ControllerDb,
    ) -> Vec<(String, String)> {
        let status = db.registry_status(info);
        let logical_id = ControllerDb::logical_id(info);
        let inferred_kind = ControllerDb::infer_kind(info);
        let input_profile = layout_profile_name(info);

        let mut rows = vec![
            ("Device".into(), js_path.to_string()),
            ("USB port".into(), info.usb_port.clone()),
            ("Kernel name".into(), info.name.clone()),
            (
                "Vendor ID".into(),
                if info.vendor_id.is_empty() {
                    "(unknown)".into()
                } else {
                    info.vendor_id.clone()
                },
            ),
            (
                "Product ID".into(),
                if info.product_id.is_empty() {
                    "(unknown)".into()
                } else {
                    info.product_id.clone()
                },
            ),
            (
                "Serial".into(),
                if info.serial.is_empty() {
                    "(none)".into()
                } else {
                    info.serial.clone()
                },
            ),
            (
                "phys".into(),
                if info.phys.is_empty() {
                    "(none)".into()
                } else {
                    info.phys.clone()
                },
            ),
            ("Logical ID".into(), logical_id),
            ("Registry status".into(), status.as_str().into()),
            ("Inferred type".into(), inferred_kind.as_str().into()),
            ("Input profile".into(), input_profile.into()),
            ("js buttons".into(), info.js_buttons.to_string()),
            ("js axes".into(), info.js_axes.to_string()),
            ("evdev keys".into(), info.evdev_key_count.to_string()),
            ("evdev abs axes".into(), info.evdev_abs_count.to_string()),
            (
                "Capture button".into(),
                if info.capture_available {
                    "yes".into()
                } else {
                    "no".into()
                },
            ),
        ];

        if let Some(entry) = db.get(info) {
            rows.push(("Saved label".into(), entry.label.clone()));
            rows.push(("Saved type".into(), entry.kind.as_str().into()));
            rows.push((
                "Setup complete".into(),
                if entry.setup_complete { "yes" } else { "no" }.into(),
            ));
            rows.push((
                "Last USB port".into(),
                if entry.last_usb_port.is_empty() {
                    "(none)".into()
                } else {
                    entry.last_usb_port.clone()
                },
            ));
        }

        rows
    }

    pub fn configure_live_hint(state: &PadState) -> String {
        if state.last_event_label.is_empty() {
            "Press any button on this controller to test input".into()
        } else if state.pressed_now.is_empty() || state.pressed_now == "—" {
            format!("Last input: {}", state.last_event_label)
        } else {
            format!(
                "Held: {}  ·  Last: {}",
                state.pressed_now, state.last_event_label
            )
        }
    }

    /// Snapshot adapter retained only for the existing setup reducer tests.
    #[cfg(test)]
    pub fn handle_input(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        info: &PadInfo,
        db: &ControllerDb,
    ) -> SetupAction {
        let previous = self.test_prev.clone();
        let mut fired = None;
        for action in LogicalAction::ALL {
            let held = pad_action_held(now, action);
            let was_held = pad_action_held(&previous, action);
            let repeat = match action {
                LogicalAction::Up => self.test_repeat.tick_up(held, frame_now),
                LogicalAction::Down => self.test_repeat.tick_down(held, frame_now),
                LogicalAction::Left => self.test_repeat.tick_left(held, frame_now),
                LogicalAction::Right => self.test_repeat.tick_right(held, frame_now),
                _ => held && !was_held,
            };
            if fired.is_none() && repeat {
                fired = Some(action);
            }
        }
        self.test_prev = now.clone();
        fired.map_or(SetupAction::None, |action| {
            self.handle_pressed(action, info, db)
        })
    }

    pub fn handle_action(
        &mut self,
        event: &InputEvent,
        _frame_now: Instant,
        info: &PadInfo,
        db: &ControllerDb,
    ) -> SetupAction {
        if event.phase == InputPhase::Released {
            return SetupAction::None;
        }
        self.handle_pressed(event.action, info, db)
    }

    fn handle_pressed(
        &mut self,
        action: LogicalAction,
        info: &PadInfo,
        db: &ControllerDb,
    ) -> SetupAction {
        if self.phase == SetupPhase::None {
            return SetupAction::None;
        }
        if !self.armed {
            self.armed = true;
            return SetupAction::None;
        }
        match self.phase {
            SetupPhase::Detected => self.handle_detected(),
            SetupPhase::NewOrExisting => self.handle_new_or_existing(action, db),
            SetupPhase::PickExisting => self.handle_pick_existing(action, db),
            SetupPhase::Configure => self.handle_configure(action, info, db),
            SetupPhase::NameKind => self.handle_name_kind(action),
            SetupPhase::None => SetupAction::None,
        }
    }

    fn handle_detected(&mut self) -> SetupAction {
        if self.trigger_status == PadRegistryStatus::MovedPort {
            self.phase = SetupPhase::NewOrExisting;
            self.list_index = 0;
        } else {
            self.phase = SetupPhase::Configure;
        }
        SetupAction::None
    }

    fn handle_new_or_existing(&mut self, action: LogicalAction, db: &ControllerDb) -> SetupAction {
        if matches!(action, LogicalAction::Left | LogicalAction::Up) {
            self.list_index = 0;
        }
        if matches!(action, LogicalAction::Right | LogicalAction::Down) {
            self.list_index = 1;
        }
        if action == LogicalAction::Activate {
            if self.list_index == 0 {
                self.phase = SetupPhase::Configure;
                SetupAction::RegisterNew
            } else if db.is_empty() {
                SetupAction::None
            } else {
                self.phase = SetupPhase::PickExisting;
                self.list_index = 0;
                SetupAction::None
            }
        } else {
            SetupAction::None
        }
    }

    fn handle_pick_existing(&mut self, action: LogicalAction, db: &ControllerDb) -> SetupAction {
        let count = db.list_entries().len();
        if count == 0 {
            self.phase = SetupPhase::NewOrExisting;
            return SetupAction::None;
        }

        if action == LogicalAction::Up {
            self.list_index = self.list_index.saturating_sub(1);
        }
        if action == LogicalAction::Down {
            self.list_index = (self.list_index + 1).min(count - 1);
        }
        if action == LogicalAction::Activate {
            let idx = self.list_index;
            self.phase = SetupPhase::Configure;
            SetupAction::ClaimExisting { list_index: idx }
        } else {
            SetupAction::None
        }
    }

    fn handle_configure(
        &mut self,
        action: LogicalAction,
        info: &PadInfo,
        db: &ControllerDb,
    ) -> SetupAction {
        if action != LogicalAction::Activate {
            return SetupAction::None;
        }
        if db.is_setup(info) {
            self.phase = SetupPhase::None;
            SetupAction::Done
        } else {
            self.begin_name_kind(info, db);
            SetupAction::None
        }
    }

    fn handle_name_kind(&mut self, action: LogicalAction) -> SetupAction {
        if action == LogicalAction::Up {
            let idx = self.draft_kind.index();
            self.draft_kind = ControllerKind::from_index(idx.saturating_sub(1));
        }
        if action == LogicalAction::Down {
            let idx = self.draft_kind.index();
            self.draft_kind =
                ControllerKind::from_index((idx + 1).min(ControllerKind::ALL.len() - 1));
        }
        if action == LogicalAction::Activate {
            self.phase = SetupPhase::None;
            return SetupAction::SaveFinish {
                label: self.draft_label.clone(),
                kind: self.draft_kind,
            };
        }
        SetupAction::None
    }
}

impl Default for SetupNav {
    fn default() -> Self {
        Self::new()
    }
}

fn default_draft_label(info: &PadInfo) -> String {
    if !info.name.is_empty() {
        return info.name.clone();
    }
    format!(
        "Controller {}:{}",
        info.vendor_id.trim_start_matches("0x"),
        info.product_id.trim_start_matches("0x")
    )
}

#[cfg(test)]
fn pad_action_held(state: &PadState, action: LogicalAction) -> bool {
    match action {
        LogicalAction::Up => state.dpad_up,
        LogicalAction::Down => state.dpad_down,
        LogicalAction::Left => state.dpad_left,
        LogicalAction::Right => state.dpad_right,
        LogicalAction::Activate => state.btn_a,
        LogicalAction::Back => state.btn_b,
        LogicalAction::Home => state.btn_home,
        LogicalAction::X => state.btn_x,
        LogicalAction::Y => state.btn_y,
        LogicalAction::L => state.btn_l,
        LogicalAction::R => state.btn_r,
        LogicalAction::Select => state.btn_select,
        LogicalAction::Start => state.btn_start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller_db::{ControllerEntry, ControllerKind};

    fn pad_info(port: &str) -> PadInfo {
        PadInfo {
            name: "Test Pad".to_string(),
            vendor_id: "0x2563".to_string(),
            product_id: "0x0575".to_string(),
            serial: "SN-A".to_string(),
            phys: format!("usb-ffb40000.usb-{port}/input0"),
            usb_port: port.to_string(),
            js_buttons: 13,
            js_axes: 6,
            evdev_key_count: 0,
            evdev_abs_count: 0,
            capture_available: false,
        }
    }

    fn press_a() -> PadState {
        PadState {
            btn_a: true,
            ..PadState::default()
        }
    }

    fn configured_entry(port: &str) -> ControllerEntry {
        ControllerEntry {
            label: "Arcade Pad".to_string(),
            kernel_name: "Test Pad".to_string(),
            kind: ControllerKind::Gamepad,
            setup_complete: true,
            last_usb_port: port.to_string(),
        }
    }

    fn empty_db(label: &str) -> ControllerDb {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-setup-nav-{label}-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        ControllerDb::load_from(&path.display().to_string())
    }

    #[test]
    fn activity_open_debounces_triggering_button_press() {
        let info = pad_info("1-1.3");
        let db = empty_db("debounce");
        let mut nav = SetupNav::new();
        nav.maybe_open(&info, 0, &db, true);

        let now = Instant::now();
        let action = nav.handle_input(&press_a(), now, &info, &db);

        assert!(matches!(action, SetupAction::None));
        assert_eq!(nav.phase, SetupPhase::Detected);
    }

    #[test]
    fn event_driven_setup_tracks_stable_device_generation() {
        let info = pad_info("1-1.3");
        let db = empty_db("stable-device");
        let mut nav = SetupNav::new();
        let device = DeviceInstanceId {
            plug_id: "usb-1-1.3".to_string(),
            generation: 7,
        };
        nav.open_for_device(PadRegistryStatus::PendingSetup, device.clone(), 2);

        assert_eq!(nav.target_device, Some(device));
        assert_eq!(nav.target_pad_idx, 2);

        let event = InputEvent {
            source: crate::input_event::InputSourceId {
                kind: crate::input_event::InputSourceKind::MainProxy,
                instance: 1,
            },
            source_epoch: crate::input_event::SourceEpoch(1),
            sequence: 1,
            press_id: crate::input_event::PressId(1),
            captured_at_us: 1,
            action: crate::input_event::LogicalAction::Start,
            phase: InputPhase::Pressed,
        };
        assert!(matches!(
            nav.handle_action(&event, Instant::now(), &info, &db),
            SetupAction::None
        ));
        assert_eq!(nav.phase, SetupPhase::Configure);
    }

    #[test]
    fn moved_port_existing_choice_with_empty_registry_stays_on_choice() {
        let info = pad_info("1-1.7");
        let db = empty_db("empty-registry");
        let mut nav = SetupNav::new();
        nav.open_for(PadRegistryStatus::MovedPort, 0);
        let now = Instant::now();
        let mut first = PadState {
            btn_start: true,
            ..PadState::default()
        };
        let _ = nav.handle_input(&first, now, &info, &db);
        assert_eq!(nav.phase, SetupPhase::NewOrExisting);

        first.btn_start = false;
        first.dpad_right = true;
        let _ = nav.handle_input(
            &first,
            now + std::time::Duration::from_millis(16),
            &info,
            &db,
        );
        assert_eq!(nav.list_index, 1);

        let action = nav.handle_input(
            &press_a(),
            now + std::time::Duration::from_millis(32),
            &info,
            &db,
        );

        assert!(matches!(action, SetupAction::None));
        assert_eq!(nav.phase, SetupPhase::NewOrExisting);
        assert_eq!(nav.list_index, 1);
    }

    #[test]
    fn configured_pad_finishes_from_configure_without_renaming() {
        let info = pad_info("1-1.3");
        let mut db = empty_db("configured");
        db.upsert(&info, configured_entry("1-1.3"));
        let mut nav = SetupNav::new();
        nav.open_for(PadRegistryStatus::PendingSetup, 0);
        nav.phase = SetupPhase::Configure;

        let action = nav.handle_input(&press_a(), Instant::now(), &info, &db);

        assert!(matches!(action, SetupAction::Done));
        assert_eq!(nav.phase, SetupPhase::None);
    }

    #[test]
    fn configure_fields_include_unknowns_and_saved_entry_details() {
        let mut info = pad_info("1-1.4");
        info.vendor_id.clear();
        info.serial.clear();
        info.phys.clear();
        let mut db = empty_db("configure-fields");
        db.upsert(&info, configured_entry("1-1.4"));

        let rows = SetupNav::configure_fields(&info, "/dev/input/js0", &db);

        assert!(rows.contains(&("Device".to_string(), "/dev/input/js0".to_string())));
        assert!(rows.contains(&("Vendor ID".to_string(), "(unknown)".to_string())));
        assert!(rows.contains(&("Serial".to_string(), "(none)".to_string())));
        assert!(rows.contains(&("Saved label".to_string(), "Arcade Pad".to_string())));
        assert!(rows.contains(&("Setup complete".to_string(), "yes".to_string())));
    }

    #[test]
    fn configure_live_hint_reports_idle_last_and_held_input() {
        assert_eq!(
            SetupNav::configure_live_hint(&PadState::default()),
            "Press any button on this controller to test input"
        );

        let mut state = PadState {
            last_event_label: "A".to_string(),
            ..PadState::default()
        };
        assert_eq!(SetupNav::configure_live_hint(&state), "Last input: A");

        state.pressed_now = "A+B".to_string();
        assert_eq!(
            SetupNav::configure_live_hint(&state),
            "Held: A+B  ·  Last: A"
        );
    }

    #[test]
    fn configure_live_hint_treats_idle_marker_as_idle() {
        let mut state = PadState {
            last_event_label: "A up (js btn 0)".to_string(),
            ..PadState::default()
        };
        state.rebuild_pressed_now();

        assert_eq!(state.pressed_now, "—");
        assert_eq!(
            SetupNav::configure_live_hint(&state),
            "Last input: A up (js btn 0)"
        );
    }

    #[test]
    fn moved_port_can_pick_existing_registry_entry() {
        let info = pad_info("1-1.9");
        let mut db = empty_db("pick-existing");
        db.upsert(&pad_info("1-1.1"), configured_entry("1-1.1"));
        let mut nav = SetupNav::new();
        nav.open_for(PadRegistryStatus::MovedPort, 0);
        let now = Instant::now();

        let _ = nav.handle_input(
            &PadState {
                btn_start: true,
                ..PadState::default()
            },
            now,
            &info,
            &db,
        );
        assert_eq!(nav.phase, SetupPhase::NewOrExisting);

        let _ = nav.handle_input(
            &PadState {
                dpad_right: true,
                ..PadState::default()
            },
            now + std::time::Duration::from_millis(20),
            &info,
            &db,
        );
        assert_eq!(nav.list_index, 1);

        let _ = nav.handle_input(
            &press_a(),
            now + std::time::Duration::from_millis(40),
            &info,
            &db,
        );
        assert_eq!(nav.phase, SetupPhase::PickExisting);

        // The confirm press that opened PickExisting is still in prev; release first.
        let _ = nav.handle_input(
            &PadState::default(),
            now + std::time::Duration::from_millis(50),
            &info,
            &db,
        );
        let action = nav.handle_input(
            &press_a(),
            now + std::time::Duration::from_millis(60),
            &info,
            &db,
        );
        assert!(matches!(
            action,
            SetupAction::ClaimExisting { list_index: 0 }
        ));
        assert_eq!(nav.phase, SetupPhase::Configure);
    }

    #[test]
    fn new_controller_name_kind_cycles_kind_and_saves() {
        let mut info = pad_info("1-1.10");
        info.name.clear();
        let db = empty_db("name-kind");
        let mut nav = SetupNav::new();
        nav.open_for(PadRegistryStatus::Unknown, 0);
        nav.phase = SetupPhase::Configure;
        let now = Instant::now();

        let _ = nav.handle_input(&press_a(), now, &info, &db);
        assert_eq!(nav.phase, SetupPhase::NameKind);
        assert_eq!(nav.draft_label, "Controller 2563:0575");
        assert_eq!(nav.draft_kind, ControllerKind::Gamepad);

        let _ = nav.handle_input(
            &PadState {
                dpad_down: true,
                ..PadState::default()
            },
            now + std::time::Duration::from_millis(20),
            &info,
            &db,
        );
        assert_eq!(nav.draft_kind, ControllerKind::FightStick);

        let action = nav.handle_input(
            &press_a(),
            now + std::time::Duration::from_millis(40),
            &info,
            &db,
        );
        match action {
            SetupAction::SaveFinish { label, kind } => {
                assert_eq!(label, "Controller 2563:0575");
                assert_eq!(kind, ControllerKind::FightStick);
            }
            _ => panic!("expected save action"),
        }
        assert_eq!(nav.phase, SetupPhase::None);
    }
}
