// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dormant, volatile on-device input latency experiment.

use crate::input_hub::monotonic_us;
use crate::launcher::{LauncherNav, Screen};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

pub(super) const INPUT_LATENCY_LAB_READY_PATH: &str =
    "/tmp/mister-magik/input-latency-lab-ready.json";
const SESSION_ENV: &str = "MISTER_INPUT_LATENCY_LAB_SESSION";
const ARM_ENV: &str = "MISTER_INPUT_LATENCY_LAB_ARM";
const MOVE_COUNT: usize = 64;
const MOVE_INTERVAL_US: u64 = 600_000;
const OBSTRUCTION_LEAD_US: u64 = 8_000;
const ARM_LEAD_US: u64 = 2_000_000;
const MAX_START_LATENESS_US: u64 = 50_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputLatencyLabArm {
    Baseline,
    Monolithic16Ms,
    Monolithic64Ms,
}

impl InputLatencyLabArm {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "baseline" => Some(Self::Baseline),
            "monolithic-16ms" => Some(Self::Monolithic16Ms),
            "monolithic-64ms" => Some(Self::Monolithic64Ms),
            _ => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Monolithic16Ms => "monolithic-16ms",
            Self::Monolithic64Ms => "monolithic-64ms",
        }
    }

    const fn work_us(self) -> u64 {
        match self {
            Self::Baseline => 0,
            Self::Monolithic16Ms => 16_000,
            Self::Monolithic64Ms => 64_000,
        }
    }
}

pub(super) struct InputLatencyLab {
    arm: Option<InputLatencyLabArm>,
    epoch_us: Option<u64>,
    next_work: usize,
    disarmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DueWork {
    NotDue,
    Run {
        ordinal: usize,
        scheduled_at_us: u64,
        started_at_us: u64,
    },
    SkipLate {
        ordinal: usize,
        scheduled_at_us: u64,
        started_at_us: u64,
    },
}

impl InputLatencyLab {
    pub(super) fn from_env() -> Self {
        let arm = std::env::var(ARM_ENV)
            .ok()
            .and_then(|value| InputLatencyLabArm::parse(&value));
        let session = std::env::var(SESSION_ENV)
            .ok()
            .filter(|path| is_volatile_path(path) && Path::new(path).is_file());
        let armed = arm.is_some() && session.is_some();
        if let Some(path) = session.as_deref().filter(|_| armed) {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(INPUT_LATENCY_LAB_READY_PATH);
        if arm.is_some() && !armed {
            crate::ui_errln!("input_latency_lab disabled: volatile session token missing");
        }
        Self {
            arm: armed.then_some(arm).flatten(),
            epoch_us: None,
            next_work: 0,
            disarmed: false,
        }
    }

    pub(super) fn arm_if_computers_ready(&mut self, nav: &LauncherNav) -> Option<Value> {
        let arm = self.arm?;
        if self.epoch_us.is_some() || self.disarmed || !computers_acorn_ready(nav) {
            return None;
        }
        let epoch_us = monotonic_us().saturating_add(ARM_LEAD_US);
        self.epoch_us = Some(epoch_us);
        let ready = json!({
            "schema": "mister-magik-input-latency-lab-ready-v1",
            "arm": arm.label(),
            "epoch_us": epoch_us,
            "move_count": MOVE_COUNT,
            "move_interval_us": MOVE_INTERVAL_US,
            "press_duration_us": 40_000,
            "work_us": arm.work_us(),
            "work_lead_us": OBSTRUCTION_LEAD_US,
        });
        if std::fs::write(INPUT_LATENCY_LAB_READY_PATH, format!("{ready}\n")).is_err() {
            self.disarmed = true;
            return Some(json!({
                "phase": "arm-failed",
                "arm": arm.label(),
                "at_us": monotonic_us(),
            }));
        }
        Some(json!({
            "phase": "armed",
            "arm": arm.label(),
            "epoch_us": epoch_us,
            "at_us": monotonic_us(),
        }))
    }

    pub(super) fn before_input_route(&mut self) -> Option<Value> {
        let arm = self.arm?;
        let epoch_us = self.epoch_us?;
        if self.disarmed || arm.work_us() == 0 || self.next_work >= MOVE_COUNT {
            return None;
        }
        match self.claim_due_work(epoch_us, monotonic_us()) {
            DueWork::NotDue => None,
            DueWork::SkipLate {
                ordinal,
                scheduled_at_us,
                started_at_us,
            } => Some(json!({
                "phase": "work-skipped-late",
                "arm": arm.label(),
                "ordinal": ordinal,
                "scheduled_at_us": scheduled_at_us,
                "started_at_us": started_at_us,
                "lateness_us": started_at_us.saturating_sub(scheduled_at_us),
            })),
            DueWork::Run {
                ordinal,
                scheduled_at_us,
                started_at_us,
            } => {
                let completed_at_us = run_bounded_cpu_work(arm.work_us());
                Some(json!({
                    "phase": "work-complete",
                    "arm": arm.label(),
                    "ordinal": ordinal,
                    "scheduled_at_us": scheduled_at_us,
                    "started_at_us": started_at_us,
                    "completed_at_us": completed_at_us,
                    "lateness_us": started_at_us.saturating_sub(scheduled_at_us),
                    "requested_work_us": arm.work_us(),
                    "work_us": completed_at_us.saturating_sub(started_at_us),
                }))
            }
        }
    }

    fn claim_due_work(&mut self, epoch_us: u64, observed_at_us: u64) -> DueWork {
        let scheduled_at_us = scheduled_work_start(epoch_us, self.next_work);
        if observed_at_us < scheduled_at_us {
            return DueWork::NotDue;
        }
        let ordinal = self.next_work;
        self.next_work += 1;
        self.disarmed = self.next_work == MOVE_COUNT;
        if observed_at_us.saturating_sub(scheduled_at_us) > MAX_START_LATENESS_US {
            DueWork::SkipLate {
                ordinal,
                scheduled_at_us,
                started_at_us: observed_at_us,
            }
        } else {
            DueWork::Run {
                ordinal,
                scheduled_at_us,
                started_at_us: observed_at_us,
            }
        }
    }

    pub(super) fn time_until_next_work(&self) -> Option<Duration> {
        let arm = self.arm?;
        let epoch_us = self.epoch_us?;
        if self.disarmed || arm.work_us() == 0 || self.next_work >= MOVE_COUNT {
            return None;
        }
        Some(Duration::from_micros(
            scheduled_work_start(epoch_us, self.next_work).saturating_sub(monotonic_us()),
        ))
    }
}

fn computers_acorn_ready(nav: &LauncherNav) -> bool {
    nav.screen == Screen::Home
        && nav.current_menu_id() == "menu:computers"
        && nav.current_menu_selected_item_id() == "menu:computers:acorn"
}

fn scheduled_work_start(epoch_us: u64, ordinal: usize) -> u64 {
    epoch_us
        .saturating_add((ordinal as u64).saturating_mul(MOVE_INTERVAL_US))
        .saturating_sub(OBSTRUCTION_LEAD_US)
}

fn run_bounded_cpu_work(work_us: u64) -> u64 {
    let started_at_us = monotonic_us();
    let deadline_us = started_at_us.saturating_add(bounded_work_us(work_us));
    let mut state = started_at_us ^ 0x9e37_79b9_7f4a_7c15;
    while monotonic_us() < deadline_us {
        for _ in 0..64 {
            state ^= state.rotate_left(13);
            state = state.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        }
        std::hint::black_box(state);
    }
    monotonic_us()
}

const fn bounded_work_us(work_us: u64) -> u64 {
    if work_us > 64_000 { 64_000 } else { work_us }
}

fn is_volatile_path(path: &str) -> bool {
    path.starts_with("/tmp/") && path.len() > "/tmp/".len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arms_are_fixed_and_bounded() {
        assert_eq!(InputLatencyLabArm::parse("baseline").unwrap().work_us(), 0);
        assert_eq!(
            InputLatencyLabArm::parse("monolithic-16ms")
                .unwrap()
                .work_us(),
            16_000
        );
        assert_eq!(
            InputLatencyLabArm::parse("monolithic-64ms")
                .unwrap()
                .work_us(),
            64_000
        );
        assert_eq!(InputLatencyLabArm::parse("monolithic-65ms"), None);
    }

    #[test]
    fn work_schedule_places_input_inside_each_obstruction() {
        let epoch = 10_000_000;
        assert_eq!(scheduled_work_start(epoch, 0), epoch - 8_000);
        assert_eq!(
            scheduled_work_start(epoch, 63),
            epoch + 63 * MOVE_INTERVAL_US - 8_000
        );
    }

    #[test]
    fn session_paths_must_be_volatile() {
        assert!(is_volatile_path("/tmp/input-latency-session"));
        assert!(!is_volatile_path("/tmp/"));
        assert!(!is_volatile_path("/media/fat/launcher.env"));
    }

    #[test]
    fn due_work_rejects_late_start_and_disarms_after_last_unit() {
        let epoch_us = 10_000_000;
        let mut lab = InputLatencyLab {
            arm: Some(InputLatencyLabArm::Monolithic16Ms),
            epoch_us: Some(epoch_us),
            next_work: 0,
            disarmed: false,
        };
        assert_eq!(
            lab.claim_due_work(epoch_us, epoch_us - OBSTRUCTION_LEAD_US - 1),
            DueWork::NotDue
        );
        assert!(matches!(
            lab.claim_due_work(epoch_us, epoch_us - OBSTRUCTION_LEAD_US),
            DueWork::Run { ordinal: 0, .. }
        ));
        lab.next_work = MOVE_COUNT - 1;
        let scheduled = scheduled_work_start(epoch_us, MOVE_COUNT - 1);
        assert!(matches!(
            lab.claim_due_work(epoch_us, scheduled + MAX_START_LATENESS_US + 1),
            DueWork::SkipLate {
                ordinal: MOVE_COUNT - 1,
                ..
            }
        ));
        assert!(lab.disarmed);
    }

    #[test]
    fn cpu_work_is_hard_capped() {
        assert_eq!(bounded_work_us(16_000), 16_000);
        assert_eq!(bounded_work_us(64_000), 64_000);
        assert_eq!(bounded_work_us(u64::MAX), 64_000);
    }
}
