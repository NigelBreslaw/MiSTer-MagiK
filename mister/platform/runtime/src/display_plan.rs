// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Main-compatible display-plan resolution for standalone device tools.

use crate::fpga::{Fpga, VideoInfo};
use mister_magik_core::display::{ResolvedDisplayPlan, RuntimeDisplayGeometry};
use std::io;

pub const RUNTIME_SETTINGS_ENV: &str = "MISTER_MAGIK_RUNTIME_SETTINGS_V1";
pub const RUNTIME_DISPLAY_ENV: &str = "MISTER_MAGIK_RUNTIME_DISPLAY_V1";

/// Standalone consumers do not inherit Main's launcher environment. Query its
/// active display mode rather than mistaking core UIO_GET_VRES timing for HDMI.
#[cfg(any(feature = "app-runtime", feature = "framebuffer-lab"))]
pub fn query_main_display_plan() -> io::Result<ResolvedDisplayPlan> {
    use crate::main_command::{self, MainCommand};
    let response = main_command::execute(&MainCommand::DisplayState)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("Main returned no active display state"))?;
    plan_from_main_response(&response)
}

#[cfg(any(feature = "app-runtime", feature = "framebuffer-lab"))]
fn plan_from_main_response(response: &str) -> io::Result<ResolvedDisplayPlan> {
    let state = crate::display_control::parse_state_response(response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    ResolvedDisplayPlan::from_mode_or_detected(&state.active_mode, None).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Main display mode {} has no resolved scanout geometry",
                state.active_mode
            ),
        )
    })
}

#[derive(Clone, Copy, Debug)]
pub struct RuntimeDisplayPlan {
    pub plan: ResolvedDisplayPlan,
    pub video: VideoInfo,
}

pub fn detect_runtime_display_plan(fpga: &mut Fpga) -> io::Result<RuntimeDisplayPlan> {
    let video = fpga.read_video_info()?;
    let detected =
        RuntimeDisplayGeometry::from_video_words(video.width, video.height, video.de_h, video.de_v);
    let settings = std::env::var(RUNTIME_SETTINGS_ENV).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("missing Main display contract {RUNTIME_SETTINGS_ENV}"),
        )
    })?;
    let display = std::env::var(RUNTIME_DISPLAY_ENV).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("missing Main display contract {RUNTIME_DISPLAY_ENV}"),
        )
    })?;
    let plan = ResolvedDisplayPlan::from_runtime_contracts(&settings, &display, detected)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid or unresolved Main display contracts",
            )
        })?;
    Ok(RuntimeDisplayPlan { plan, video })
}

#[cfg(all(test, any(feature = "app-runtime", feature = "framebuffer-lab")))]
mod tests {
    use super::*;

    #[test]
    fn main_hdmi_contract_keeps_source_half_size_and_destination_full_size() {
        let plan = plan_from_main_response(
            "ok DisplayV1 schema=1 active=hdmi-1920x1080p60 pending=none phase=idle",
        )
        .unwrap();
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
        assert_eq!((plan.scan_w, plan.scan_h), (1920, 1080));
    }

    #[test]
    fn pixel_repeated_mode_uses_scan_coordinates_and_active_not_pending_mode() {
        let plan = plan_from_main_response("ok DisplayV1 schema=1 active=hdmi-2560x1440p60 pending=hdmi-1920x1080p60 phase=provisional").unwrap();
        assert_eq!((plan.output_w, plan.output_h), (2560, 1440));
        assert_eq!((plan.scan_w, plan.scan_h), (1280, 1440));
    }

    #[test]
    fn unresolved_mode_does_not_fall_back_to_core_or_source_timing() {
        assert!(plan_from_main_response("ok DisplayV1 schema=1 active=auto").is_err());
        assert!(plan_from_main_response("ok DisplayV1 schema=2 active=hdmi-1920x1080p60").is_err());
    }
}
