//! Controller setup flow — detect unknown / moved pads and offer rebinding.

use crate::controller_db::{ControllerDb, ControllerKind, PadRegistryStatus};
use crate::input::PadPool;
use crate::input::{layout_profile_name, PadInfo, PadState};
use crate::input_repeat::RepeatNav;
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
    pub list_index: usize,
    pub draft_label: String,
    pub draft_kind: ControllerKind,
    repeat: RepeatNav,
    prev: PadState,
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

impl SetupNav {
    pub fn new() -> Self {
        Self {
            phase: SetupPhase::None,
            trigger_status: PadRegistryStatus::Unknown,
            target_pad_idx: 0,
            list_index: 0,
            draft_label: String::new(),
            draft_kind: ControllerKind::Unknown,
            repeat: RepeatNav::default(),
            prev: PadState::default(),
            armed: false,
        }
    }

    pub fn open_for(&mut self, status: PadRegistryStatus, pad_idx: usize) {
        self.trigger_status = status;
        self.target_pad_idx = pad_idx;
        self.phase = SetupPhase::Detected;
        self.list_index = 0;
        // Startup / programmatic open — accept input on the first press.
        self.armed = true;
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
    pub fn advance_to_next_pad(&mut self, pad: &PadPool) {
        self.phase = SetupPhase::None;
        if let Some(idx) = pad.index_needing_setup() {
            let status = pad.db().registry_status(pad.info_at(idx));
            eprintln!("controller setup: advancing to pad {idx} ({status:?})");
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
        } else if state.pressed_now.is_empty() {
            format!("Last input: {}", state.last_event_label)
        } else {
            format!(
                "Held: {}  ·  Last: {}",
                state.pressed_now, state.last_event_label
            )
        }
    }

    /// Handle input while setup overlay is visible. Returns an action for the caller.
    pub fn handle_input(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        info: &PadInfo,
        db: &ControllerDb,
    ) -> SetupAction {
        if self.phase == SetupPhase::None {
            self.prev = now.clone();
            return SetupAction::None;
        }

        if !self.armed {
            self.armed = true;
            self.prev = now.clone();
            return SetupAction::None;
        }

        let action = match self.phase {
            SetupPhase::Detected => self.handle_detected(now, info),
            SetupPhase::NewOrExisting => self.handle_new_or_existing(now, frame_now, db),
            SetupPhase::PickExisting => self.handle_pick_existing(now, frame_now, db),
            SetupPhase::Configure => self.handle_configure(now, info, db),
            SetupPhase::NameKind => self.handle_name_kind(now, frame_now),
            SetupPhase::None => SetupAction::None,
        };

        self.prev = now.clone();
        action
    }

    fn handle_detected(&mut self, now: &PadState, _info: &PadInfo) -> SetupAction {
        if !any_control_rising(now, &self.prev) {
            return SetupAction::None;
        }
        if self.trigger_status == PadRegistryStatus::MovedPort {
            self.phase = SetupPhase::NewOrExisting;
            self.list_index = 0;
        } else {
            self.phase = SetupPhase::Configure;
        }
        SetupAction::None
    }

    fn handle_new_or_existing(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        db: &ControllerDb,
    ) -> SetupAction {
        if self.repeat.tick_left(now.dpad_left, frame_now)
            || self.repeat.tick_up(now.dpad_up, frame_now)
        {
            self.list_index = 0;
        }
        if self.repeat.tick_right(now.dpad_right, frame_now)
            || self.repeat.tick_down(now.dpad_down, frame_now)
        {
            self.list_index = 1;
        }
        if rising(now.btn_a, self.prev.btn_a) {
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

    fn handle_pick_existing(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        db: &ControllerDb,
    ) -> SetupAction {
        let count = db.list_entries().len();
        if count == 0 {
            self.phase = SetupPhase::NewOrExisting;
            return SetupAction::None;
        }

        if self.repeat.tick_up(now.dpad_up, frame_now) {
            self.list_index = self.list_index.saturating_sub(1);
        }
        if self.repeat.tick_down(now.dpad_down, frame_now) {
            self.list_index = (self.list_index + 1).min(count - 1);
        }
        if rising(now.btn_a, self.prev.btn_a) {
            let idx = self.list_index;
            self.phase = SetupPhase::Configure;
            SetupAction::ClaimExisting { list_index: idx }
        } else {
            SetupAction::None
        }
    }

    fn handle_configure(
        &mut self,
        now: &PadState,
        info: &PadInfo,
        db: &ControllerDb,
    ) -> SetupAction {
        if !rising(now.btn_a, self.prev.btn_a) {
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

    fn handle_name_kind(&mut self, now: &PadState, frame_now: Instant) -> SetupAction {
        if self.repeat.tick_up(now.dpad_up, frame_now) {
            let idx = self.draft_kind.index();
            self.draft_kind = ControllerKind::from_index(idx.saturating_sub(1));
        }
        if self.repeat.tick_down(now.dpad_down, frame_now) {
            let idx = self.draft_kind.index();
            self.draft_kind =
                ControllerKind::from_index((idx + 1).min(ControllerKind::ALL.len() - 1));
        }
        if rising(now.btn_a, self.prev.btn_a) {
            self.phase = SetupPhase::None;
            return SetupAction::SaveFinish {
                label: self.draft_label.clone(),
                kind: self.draft_kind,
            };
        }
        SetupAction::None
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

fn rising(now: bool, prev: bool) -> bool {
    now && !prev
}

fn any_control_rising(now: &PadState, prev: &PadState) -> bool {
    rising(now.dpad_up, prev.dpad_up)
        || rising(now.dpad_down, prev.dpad_down)
        || rising(now.dpad_left, prev.dpad_left)
        || rising(now.dpad_right, prev.dpad_right)
        || rising(now.btn_a, prev.btn_a)
        || rising(now.btn_b, prev.btn_b)
        || rising(now.btn_x, prev.btn_x)
        || rising(now.btn_y, prev.btn_y)
        || rising(now.btn_l, prev.btn_l)
        || rising(now.btn_r, prev.btn_r)
        || rising(now.btn_zl, prev.btn_zl)
        || rising(now.btn_zr, prev.btn_zr)
        || rising(now.btn_select, prev.btn_select)
        || rising(now.btn_start, prev.btn_start)
        || rising(now.btn_l3, prev.btn_l3)
        || rising(now.btn_r3, prev.btn_r3)
        || rising(now.btn_home, prev.btn_home)
        || rising(now.btn_capture, prev.btn_capture)
}
