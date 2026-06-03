//! Controller setup flow — detect unknown / moved pads and offer rebinding.

use crate::controller_db::{ControllerDb, PadRegistryStatus};
use crate::input::{PadInfo, PadState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupPhase {
    None = 0,
    /// "New input detected" — press any button to continue.
    Detected = 1,
    /// Moved port: new controller vs pick from registry.
    NewOrExisting = 2,
    /// Scroll list of saved controllers and confirm.
    PickExisting = 3,
}

pub struct SetupNav {
    pub phase: SetupPhase,
    /// Registry status that triggered the current flow.
    pub trigger_status: PadRegistryStatus,
    /// Which pad in the pool this dialog refers to.
    pub target_pad_idx: usize,
    pub list_index: usize,
    prev: PadState,
    /// Ignore the triggering edge on the same frame we opened from pad activity.
    armed: bool,
}

pub enum SetupAction {
    None,
    /// User confirmed this is a new controller identity (MovedPort → "New controller").
    RegisterNew,
    /// User picked an existing registry entry by index in `list_entries()`.
    ClaimExisting { list_index: usize },
}

impl SetupNav {
    pub fn new() -> Self {
        Self {
            phase: SetupPhase::None,
            trigger_status: PadRegistryStatus::Unknown,
            target_pad_idx: 0,
            list_index: 0,
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
        }
    }

    /// Handle input while setup overlay is visible. Returns an action for the caller.
    pub fn handle_input(
        &mut self,
        now: &PadState,
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
            SetupPhase::NewOrExisting => self.handle_new_or_existing(now, db),
            SetupPhase::PickExisting => self.handle_pick_existing(now, db),
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
        }
        // Unknown / PendingSetup — stay in setup; wizard TBD. Do not write DB yet.
        SetupAction::None
    }

    fn handle_new_or_existing(&mut self, now: &PadState, db: &ControllerDb) -> SetupAction {
        if rising(now.dpad_left, self.prev.dpad_left) || rising(now.dpad_up, self.prev.dpad_up) {
            self.list_index = 0;
        }
        if rising(now.dpad_right, self.prev.dpad_right)
            || rising(now.dpad_down, self.prev.dpad_down)
        {
            self.list_index = 1;
        }
        if rising(now.btn_a, self.prev.btn_a) {
            if self.list_index == 0 {
                self.phase = SetupPhase::None;
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

    fn handle_pick_existing(&mut self, now: &PadState, db: &ControllerDb) -> SetupAction {
        let count = db.list_entries().len();
        if count == 0 {
            self.phase = SetupPhase::NewOrExisting;
            return SetupAction::None;
        }

        if rising(now.dpad_up, self.prev.dpad_up) {
            self.list_index = self.list_index.saturating_sub(1);
        }
        if rising(now.dpad_down, self.prev.dpad_down) {
            self.list_index = (self.list_index + 1).min(count - 1);
        }
        if rising(now.btn_a, self.prev.btn_a) {
            let idx = self.list_index;
            self.phase = SetupPhase::None;
            SetupAction::ClaimExisting { list_index: idx }
        } else {
            SetupAction::None
        }
    }
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
