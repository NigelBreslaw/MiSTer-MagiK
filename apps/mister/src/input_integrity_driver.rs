// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded qualification-only uinput producer. Main consumes this device and
//! forwards resolved actions through its production proxy v2 path.

use std::io::{self, Write};
use std::time::Duration;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const SYN_REPORT: u16 = 0;
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_DEV_CREATE: libc::c_ulong = 0x0000_5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x0000_5502;
const UINPUT_USER_DEV_SIZE: usize = 1116;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DriverPlan {
    key_code: u16,
    pulse_ms: u64,
    gap_ms: u64,
    count: u32,
}

impl DriverPlan {
    fn parse(args: &[String]) -> Result<Self, String> {
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
            count,
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

pub(crate) fn run(args: &[String]) {
    let plan = match DriverPlan::parse(args) {
        Ok(plan) => plan,
        Err(error) => {
            crate::ui_errln!("input-integrity-driver: {error}");
            std::process::exit(2);
        }
    };
    match UinputDevice::create(plan.key_code).and_then(|mut device| device.run(plan)) {
        Ok(()) => crate::ui_logln!(
            "{}",
            serde_json::json!({
                "schema": "mister-magik-input-integrity-driver-v1",
                "status": "passed",
                "pulse_ms": plan.pulse_ms,
                "gap_ms": plan.gap_ms,
                "count": plan.count,
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
}

impl UinputDevice {
    fn create(key_code: u16) -> io::Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")?;
        ioctl_int(&file, UI_SET_EVBIT, i32::from(EV_KEY))?;
        ioctl_int(&file, UI_SET_KEYBIT, i32::from(key_code))?;
        let mut descriptor = [0_u8; UINPUT_USER_DEV_SIZE];
        let name = b"MiSTer MagiK input integrity\0";
        descriptor[..name.len()].copy_from_slice(name);
        descriptor[80..82].copy_from_slice(&0x03_u16.to_ne_bytes());
        descriptor[82..84].copy_from_slice(&0x1209_u16.to_ne_bytes());
        descriptor[84..86].copy_from_slice(&0x4d4b_u16.to_ne_bytes());
        descriptor[86..88].copy_from_slice(&1_u16.to_ne_bytes());
        file.write_all(&descriptor)?;
        ioctl_no_arg(&file, UI_DEV_CREATE)?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(Self { file })
    }

    fn run(&mut self, plan: DriverPlan) -> io::Result<()> {
        for index in 0..plan.count {
            self.emit(EV_KEY, plan.key_code, 1)?;
            self.emit(EV_SYN, SYN_REPORT, 0)?;
            std::thread::sleep(Duration::from_millis(plan.pulse_ms));
            self.emit(EV_KEY, plan.key_code, 0)?;
            self.emit(EV_SYN, SYN_REPORT, 0)?;
            if index + 1 < plan.count {
                std::thread::sleep(Duration::from_millis(plan.gap_ms));
            }
        }
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

impl Drop for UinputDevice {
    fn drop(&mut self) {
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
                count: 8,
            }
        );
        assert!(DriverPlan::parse(&["right", "0", "1", "1"].map(str::to_string)).is_err());
    }
}
