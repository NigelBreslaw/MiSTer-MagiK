// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded qualification-only uinput producer. Main consumes this device and
//! forwards resolved actions through its production proxy v2 path.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS_HAT0X: u16 = 16;
const ABS_HAT0Y: u16 = 17;
const BTN_SOUTH: u16 = 304;
const BTN_EAST: u16 = 305;
const BTN_MODE: u16 = 316;
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;
const UI_DEV_CREATE: libc::c_ulong = 0x0000_5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x0000_5502;
const UINPUT_USER_DEV_SIZE: usize = 1116;
const MAIN_INPUT_DEVICE_SETTLE_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DriverPlan {
    key_code: u16,
    pulse_ms: u64,
    gap_ms: u64,
    start_delay_ms: u64,
    start_at_us: u64,
    count: u32,
    qualification: bool,
    cpu_load: bool,
    sequence: DriverSequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DriverSequence {
    Pulses,
    TransitionRight,
    TransitionBack,
    ComputersSweep,
    ComputersEnterSweep,
    ComputersRoundTrip,
    ComputersEnterRoundTrip,
    LauncherResponse,
}

impl DriverPlan {
    fn parse(args: &[String]) -> Result<Self, String> {
        if matches!(
            args.first().map(String::as_str),
            Some("qualification" | "qualification-load" | "qualification-right")
        ) {
            let mode = args.first().map(String::as_str);
            return Ok(Self {
                key_code: if mode == Some("qualification-right") {
                    106
                } else {
                    108
                },
                pulse_ms: 0,
                gap_ms: 0,
                start_delay_ms: 0,
                start_at_us: 0,
                count: 109,
                qualification: true,
                cpu_load: mode == Some("qualification-load"),
                sequence: DriverSequence::Pulses,
            });
        }
        if args.first().map(String::as_str) == Some("computers-sweep") {
            let interval_ms = parse_bounded(args.get(1), "interval_ms", 50, 600)?;
            let start_delay_ms = parse_bounded(args.get(2), "start_delay_ms", 0, interval_ms - 1)?;
            if args.len() != 3 {
                return Err("usage: computers-sweep interval_ms start_delay_ms".to_string());
            }
            return Ok(Self {
                key_code: 106,
                pulse_ms: 40,
                gap_ms: interval_ms - 40,
                start_delay_ms,
                start_at_us: 0,
                count: 8,
                qualification: false,
                cpu_load: false,
                sequence: DriverSequence::ComputersSweep,
            });
        }
        if args.first().map(String::as_str) == Some("computers-enter-sweep") {
            let interval_ms = parse_bounded(args.get(1), "interval_ms", 50, 600)?;
            let start_delay_ms = parse_bounded(args.get(2), "start_delay_ms", 0, interval_ms - 1)?;
            if args.len() != 3 {
                return Err("usage: computers-enter-sweep interval_ms start_delay_ms".to_string());
            }
            return Ok(Self {
                key_code: 106,
                pulse_ms: 40,
                gap_ms: interval_ms - 40,
                start_delay_ms,
                start_at_us: 0,
                count: 9,
                qualification: false,
                cpu_load: false,
                sequence: DriverSequence::ComputersEnterSweep,
            });
        }
        if args.first().map(String::as_str) == Some("computers-round-trip") {
            let interval_ms = parse_bounded(args.get(1), "interval_ms", 50, 600)?;
            let cycles = parse_bounded(args.get(2), "cycles", 1, 8)? as u32;
            if args.len() != 3 {
                return Err("usage: computers-round-trip interval_ms cycles".to_string());
            }
            return Ok(Self {
                key_code: 106,
                pulse_ms: 40,
                gap_ms: interval_ms - 40,
                start_delay_ms: 0,
                start_at_us: 0,
                count: cycles * 16,
                qualification: false,
                cpu_load: false,
                sequence: DriverSequence::ComputersRoundTrip,
            });
        }
        if args.first().map(String::as_str) == Some("computers-enter-round-trip") {
            let interval_ms = parse_bounded(args.get(1), "interval_ms", 50, 600)?;
            let cycles = parse_bounded(args.get(2), "cycles", 1, 8)? as u32;
            if args.len() != 3 {
                return Err("usage: computers-enter-round-trip interval_ms cycles".to_string());
            }
            return Ok(Self {
                key_code: 106,
                pulse_ms: 40,
                gap_ms: interval_ms - 40,
                start_delay_ms: 0,
                start_at_us: 0,
                count: cycles * 16 + 1,
                qualification: false,
                cpu_load: false,
                sequence: DriverSequence::ComputersEnterRoundTrip,
            });
        }
        if args.first().map(String::as_str) == Some("computers-round-trip-at") {
            let start_at_us = parse_bounded(args.get(1), "start_at_us", 1, u64::MAX)?;
            let interval_ms = parse_bounded(args.get(2), "interval_ms", 50, 600)?;
            let cycles = parse_bounded(args.get(3), "cycles", 1, 8)? as u32;
            if args.len() != 4 {
                return Err(
                    "usage: computers-round-trip-at start_at_us interval_ms cycles".to_string(),
                );
            }
            return Ok(Self {
                key_code: 106,
                pulse_ms: 40,
                gap_ms: interval_ms - 40,
                start_delay_ms: 0,
                start_at_us,
                count: cycles * 16,
                qualification: false,
                cpu_load: false,
                sequence: DriverSequence::ComputersRoundTrip,
            });
        }
        if matches!(
            args.first().map(String::as_str),
            Some("transition-right" | "transition-back" | "launcher-response")
        ) {
            let launcher_response = args.first().map(String::as_str) == Some("launcher-response");
            return Ok(Self {
                key_code: 28,
                pulse_ms: 10,
                gap_ms: 50,
                start_delay_ms: 0,
                start_at_us: 0,
                count: if launcher_response { 17 } else { 1 },
                qualification: false,
                cpu_load: false,
                sequence: match args.first().map(String::as_str) {
                    Some("transition-right") => DriverSequence::TransitionRight,
                    Some("transition-back") => DriverSequence::TransitionBack,
                    _ => DriverSequence::LauncherResponse,
                },
            });
        }
        let key_code = match args.first().map(String::as_str) {
            Some("up") => 103,
            Some("left") => 105,
            Some("right") => 106,
            Some("down") => 108,
            Some("a") => 28,
            Some("back") => 1,
            Some("home") => 139,
            _ => return Err("key must be up|down|left|right|a|back|home".to_string()),
        };
        let pulse_ms = parse_bounded(args.get(1), "pulse_ms", 1, 2_000)?;
        let count = parse_bounded(args.get(2), "count", 1, 256)? as u32;
        let gap_ms = parse_bounded(args.get(3), "gap_ms", 1, 2_000)?;
        Ok(Self {
            key_code,
            pulse_ms,
            gap_ms,
            start_delay_ms: 0,
            start_at_us: 0,
            count,
            qualification: false,
            cpu_load: false,
            sequence: DriverSequence::Pulses,
        })
    }
}

fn parse_bounded(
    value: Option<&String>,
    label: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, String> {
    let parsed = value
        .ok_or_else(|| format!("missing {label}"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid {label}"))?;
    (minimum..=maximum)
        .contains(&parsed)
        .then_some(parsed)
        .ok_or_else(|| format!("{label} must be {minimum}..={maximum}"))
}

pub fn run(args: &[String]) {
    let plan = match DriverPlan::parse(args) {
        Ok(plan) => plan,
        Err(error) => {
            crate::ui_errln!("input-integrity-driver: {error}");
            std::process::exit(2);
        }
    };
    match UinputDevice::create().and_then(|mut device| {
        device.run(plan)?;
        Ok(std::mem::take(&mut device.pulses))
    }) {
        Ok(pulses) => crate::ui_logln!(
            "{}",
            serde_json::json!({
                "schema": "mister-magik-input-integrity-driver-v1",
                "status": "passed",
                "pulse_ms": plan.pulse_ms,
                "gap_ms": plan.gap_ms,
                "start_delay_ms": plan.start_delay_ms,
                "start_at_us": plan.start_at_us,
                "count": plan.count,
                "qualification": plan.qualification,
                "cpu_load": plan.cpu_load,
                "sequence": format!("{:?}", plan.sequence).to_ascii_lowercase(),
                "pulses": pulses,
            })
        ),
        Err(error) => {
            crate::ui_errln!("input-integrity-driver: {error}");
            std::process::exit(1);
        }
    }
}

struct UinputDevice {
    file: std::fs::File,
    pulses: Vec<DriverPulseTrace>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct DriverPulseTrace {
    ordinal: usize,
    key_code: u16,
    scheduled_at_us: Option<u64>,
    write_started_at_us: u64,
    emitted_at_us: u64,
    released_at_us: u64,
}

impl UinputDevice {
    fn create() -> io::Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")?;
        ioctl_int(&file, UI_SET_EVBIT, i32::from(EV_KEY))?;
        ioctl_int(&file, UI_SET_EVBIT, i32::from(EV_ABS))?;
        for code in [BTN_SOUTH, BTN_EAST, BTN_MODE] {
            ioctl_int(&file, UI_SET_KEYBIT, i32::from(code))?;
        }
        for code in [ABS_HAT0X, ABS_HAT0Y] {
            ioctl_int(&file, UI_SET_ABSBIT, i32::from(code))?;
        }
        let mut descriptor = [0_u8; UINPUT_USER_DEV_SIZE];
        let name = b"MiSTer MagiK input integrity\0";
        descriptor[..name.len()].copy_from_slice(name);
        descriptor[80..82].copy_from_slice(&0x03_u16.to_ne_bytes());
        descriptor[82..84].copy_from_slice(&0x1209_u16.to_ne_bytes());
        descriptor[84..86].copy_from_slice(&0x4d4b_u16.to_ne_bytes());
        descriptor[86..88].copy_from_slice(&1_u16.to_ne_bytes());
        set_abs_range(&mut descriptor, ABS_HAT0X, -1, 1);
        set_abs_range(&mut descriptor, ABS_HAT0Y, -1, 1);
        file.write_all(&descriptor)?;
        ioctl_no_arg(&file, UI_DEV_CREATE)?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(Self {
            file,
            pulses: Vec::new(),
        })
    }

    fn run(&mut self, plan: DriverPlan) -> io::Result<()> {
        if plan.qualification {
            let stop = Arc::new(AtomicBool::new(false));
            let load_thread = plan.cpu_load.then(|| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            });
            for index in 0..100 {
                self.pulse(plan.key_code, [5, 10, 20, 40][index % 4])?;
                std::thread::sleep(Duration::from_millis(10));
            }
            for _ in 0..8 {
                self.pulse(plan.key_code, 5)?;
                std::thread::sleep(Duration::from_millis(5));
            }
            self.pulse(plan.key_code, 500)?;
            stop.store(true, Ordering::Relaxed);
            if let Some(thread) = load_thread {
                let _ = thread.join();
            }
            return Ok(());
        }
        match plan.sequence {
            DriverSequence::TransitionRight | DriverSequence::TransitionBack => {
                self.pulse(28, 10)?;
                std::thread::sleep(Duration::from_millis(40));
                return self.pulse(
                    if plan.sequence == DriverSequence::TransitionRight {
                        106
                    } else {
                        1
                    },
                    10,
                );
            }
            DriverSequence::LauncherResponse => {
                std::thread::sleep(Duration::from_millis(MAIN_INPUT_DEVICE_SETTLE_MS));
                for _ in 0..4 {
                    for (key_code, pulse_ms) in [(106, 5), (105, 10), (108, 20), (103, 40)] {
                        self.pulse(key_code, pulse_ms)?;
                        std::thread::sleep(Duration::from_millis(50 - pulse_ms));
                    }
                }
                return self.pulse(106, 500);
            }
            DriverSequence::ComputersSweep | DriverSequence::ComputersEnterSweep => {
                if plan.sequence == DriverSequence::ComputersEnterSweep {
                    self.pulse(28, 10)?;
                    std::thread::sleep(Duration::from_millis(MAIN_INPUT_DEVICE_SETTLE_MS));
                }
                std::thread::sleep(Duration::from_millis(plan.start_delay_ms));
                for index in 0..8 {
                    self.pulse(
                        if plan.sequence == DriverSequence::ComputersEnterSweep {
                            computers_sweep_key(index)
                        } else {
                            106
                        },
                        plan.pulse_ms,
                    )?;
                    if index < 7 {
                        std::thread::sleep(Duration::from_millis(plan.gap_ms));
                    }
                }
                if plan.sequence == DriverSequence::ComputersEnterSweep {
                    std::thread::sleep(Duration::from_millis(500));
                }
                return Ok(());
            }
            DriverSequence::ComputersRoundTrip | DriverSequence::ComputersEnterRoundTrip => {
                let measured_count = if plan.sequence == DriverSequence::ComputersEnterRoundTrip {
                    self.pulse(28, 10)?;
                    std::thread::sleep(Duration::from_millis(MAIN_INPUT_DEVICE_SETTLE_MS));
                    plan.count - 1
                } else {
                    plan.count
                };
                for index in 0..measured_count {
                    if plan.start_at_us > 0 {
                        let scheduled_at_us = plan.start_at_us.saturating_add(
                            u64::from(index)
                                .saturating_mul(plan.pulse_ms.saturating_add(plan.gap_ms))
                                .saturating_mul(1_000),
                        );
                        wait_until_monotonic(scheduled_at_us, index == 0)?;
                        self.pulse_at(
                            computers_round_trip_key(index),
                            plan.pulse_ms,
                            Some(scheduled_at_us),
                        )?;
                    } else {
                        self.pulse(computers_round_trip_key(index), plan.pulse_ms)?;
                    }
                    if plan.start_at_us == 0 && index + 1 < measured_count {
                        std::thread::sleep(Duration::from_millis(plan.gap_ms));
                    }
                }
                if plan.sequence == DriverSequence::ComputersEnterRoundTrip {
                    std::thread::sleep(Duration::from_millis(500));
                }
                return Ok(());
            }
            DriverSequence::Pulses => {}
        }
        for index in 0..plan.count {
            self.pulse(plan.key_code, plan.pulse_ms)?;
            if index + 1 < plan.count {
                std::thread::sleep(Duration::from_millis(plan.gap_ms));
            }
        }
        Ok(())
    }

    fn pulse(&mut self, key_code: u16, duration_ms: u64) -> io::Result<()> {
        self.pulse_at(key_code, duration_ms, None)
    }

    fn pulse_at(
        &mut self,
        key_code: u16,
        duration_ms: u64,
        scheduled_at_us: Option<u64>,
    ) -> io::Result<()> {
        let (event_type, code, pressed) = match key_code {
            103 => (EV_ABS, ABS_HAT0Y, -1),
            105 => (EV_ABS, ABS_HAT0X, -1),
            106 => (EV_ABS, ABS_HAT0X, 1),
            108 => (EV_ABS, ABS_HAT0Y, 1),
            28 => (EV_KEY, BTN_EAST, 1),
            1 => (EV_KEY, BTN_SOUTH, 1),
            139 => (EV_KEY, BTN_MODE, 1),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unknown control",
                ));
            }
        };
        let write_started_at_us = crate::input_hub::monotonic_us();
        self.emit(event_type, code, pressed)?;
        self.emit(EV_SYN, SYN_REPORT, 0)?;
        let emitted_at_us = crate::input_hub::monotonic_us();
        std::thread::sleep(Duration::from_millis(duration_ms));
        self.emit(event_type, code, 0)?;
        self.emit(EV_SYN, SYN_REPORT, 0)?;
        let released_at_us = crate::input_hub::monotonic_us();
        self.pulses.push(DriverPulseTrace {
            ordinal: self.pulses.len(),
            key_code,
            scheduled_at_us,
            write_started_at_us,
            emitted_at_us,
            released_at_us,
        });
        Ok(())
    }

    fn emit(&mut self, event_type: u16, code: u16, value: i32) -> io::Result<()> {
        let mut bytes = vec![
            0_u8;
            if cfg!(target_pointer_width = "64") {
                24
            } else {
                16
            }
        ];
        let offset = bytes.len() - 8;
        bytes[offset..offset + 2].copy_from_slice(&event_type.to_ne_bytes());
        bytes[offset + 2..offset + 4].copy_from_slice(&code.to_ne_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&value.to_ne_bytes());
        self.file.write_all(&bytes)
    }
}

fn wait_until_monotonic(target_us: u64, reject_late_start: bool) -> io::Result<()> {
    loop {
        let now_us = crate::input_hub::monotonic_us();
        if now_us >= target_us {
            if late_start_exceeded(target_us, now_us, reject_late_start) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "scheduled input epoch is more than 50 ms late",
                ));
            }
            return Ok(());
        }
        let remaining_us = target_us - now_us;
        if remaining_us > 2_000 {
            std::thread::sleep(Duration::from_micros(remaining_us - 1_000));
        } else {
            std::thread::yield_now();
        }
    }
}

fn late_start_exceeded(target_us: u64, now_us: u64, reject_late_start: bool) -> bool {
    reject_late_start && now_us.saturating_sub(target_us) > 50_000
}

fn computers_round_trip_key(index: u32) -> u16 {
    if (index / 8).is_multiple_of(2) {
        106
    } else {
        105
    }
}

fn computers_sweep_key(index: u32) -> u16 {
    if index.is_multiple_of(2) { 106 } else { 105 }
}

fn set_abs_range(descriptor: &mut [u8; UINPUT_USER_DEV_SIZE], code: u16, min: i32, max: i32) {
    let index = usize::from(code);
    descriptor[92 + index * 4..96 + index * 4].copy_from_slice(&max.to_ne_bytes());
    descriptor[348 + index * 4..352 + index * 4].copy_from_slice(&min.to_ne_bytes());
}

impl Drop for UinputDevice {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_millis(100));
        let _ = ioctl_no_arg(&self.file, UI_DEV_DESTROY);
    }
}

fn ioctl_int(file: &std::fs::File, request: libc::c_ulong, value: i32) -> io::Result<()> {
    let result = unsafe { libc::ioctl(std::os::fd::AsRawFd::as_raw_fd(file), request, value) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_no_arg(file: &std::fs::File, request: libc::c_ulong) -> io::Result<()> {
    let result = unsafe { libc::ioctl(std::os::fd::AsRawFd::as_raw_fd(file), request) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_plan_is_strict_and_bounded() {
        let args = ["right", "5", "8", "10"].map(str::to_string);
        assert_eq!(
            DriverPlan::parse(&args).unwrap(),
            DriverPlan {
                key_code: 106,
                pulse_ms: 5,
                gap_ms: 10,
                start_delay_ms: 0,
                start_at_us: 0,
                count: 8,
                qualification: false,
                cpu_load: false,
                sequence: DriverSequence::Pulses,
            }
        );
        assert!(DriverPlan::parse(&["right", "0", "1", "1"].map(str::to_string)).is_err());
        assert!(
            DriverPlan::parse(&["qualification".to_string()])
                .unwrap()
                .qualification
        );
        assert!(
            DriverPlan::parse(&["qualification-load".to_string()])
                .unwrap()
                .cpu_load
        );
        let horizontal = DriverPlan::parse(&["qualification-right".to_string()]).unwrap();
        assert!(horizontal.qualification);
        assert_eq!(horizontal.key_code, 106);
        assert!(!horizontal.cpu_load);
        assert_eq!(
            DriverPlan::parse(&["launcher-response".to_string()])
                .unwrap()
                .sequence,
            DriverSequence::LauncherResponse
        );
        assert_eq!(
            DriverPlan::parse(&["launcher-response".to_string()])
                .unwrap()
                .count,
            17
        );
        assert_eq!(
            DriverPlan::parse(&["computers-sweep", "57", "7"].map(str::to_string))
                .unwrap()
                .sequence,
            DriverSequence::ComputersSweep
        );
        assert!(DriverPlan::parse(&["computers-sweep".to_string()]).is_err());
        let sweep =
            DriverPlan::parse(&["computers-sweep", "50", "13"].map(str::to_string)).unwrap();
        assert_eq!(sweep.pulse_ms, 40);
        assert_eq!(sweep.gap_ms, 10);
        assert_eq!(sweep.start_delay_ms, 13);
        let enter_sweep =
            DriverPlan::parse(&["computers-enter-sweep", "57", "7"].map(str::to_string)).unwrap();
        assert_eq!(enter_sweep.sequence, DriverSequence::ComputersEnterSweep);
        assert_eq!(enter_sweep.count, 9);
        assert_eq!(enter_sweep.start_delay_ms, 7);
        assert_eq!(
            (0..8).map(computers_sweep_key).collect::<Vec<_>>(),
            vec![106, 105, 106, 105, 106, 105, 106, 105]
        );
        let isolated =
            DriverPlan::parse(&["computers-sweep", "600", "455"].map(str::to_string)).unwrap();
        assert_eq!(isolated.gap_ms, 560);
        assert_eq!(isolated.start_delay_ms, 455);
        assert!(DriverPlan::parse(&["computers-sweep", "601", "0"].map(str::to_string)).is_err());
        let round_trip =
            DriverPlan::parse(&["computers-round-trip", "600", "4"].map(str::to_string)).unwrap();
        assert_eq!(round_trip.sequence, DriverSequence::ComputersRoundTrip);
        assert_eq!(round_trip.pulse_ms, 40);
        assert_eq!(round_trip.gap_ms, 560);
        assert_eq!(round_trip.count, 64);
        let enter_round_trip =
            DriverPlan::parse(&["computers-enter-round-trip", "600", "2"].map(str::to_string))
                .unwrap();
        assert_eq!(
            enter_round_trip.sequence,
            DriverSequence::ComputersEnterRoundTrip
        );
        assert_eq!(enter_round_trip.count, 33);
        let scheduled = DriverPlan::parse(
            &["computers-round-trip-at", "12345678", "600", "4"].map(str::to_string),
        )
        .unwrap();
        assert_eq!(scheduled.start_at_us, 12_345_678);
        assert_eq!(scheduled.count, 64);
        assert_eq!(scheduled.pulse_ms + scheduled.gap_ms, 600);
        assert_eq!(computers_round_trip_key(0), 106);
        assert_eq!(computers_round_trip_key(7), 106);
        assert_eq!(computers_round_trip_key(8), 105);
        assert_eq!(computers_round_trip_key(15), 105);
        assert_eq!(computers_round_trip_key(16), 106);
        assert!(!late_start_exceeded(1_000_000, 1_050_000, true));
        assert!(late_start_exceeded(1_000_000, 1_050_001, true));
        assert!(!late_start_exceeded(1_000_000, 2_000_000, false));
    }
}
