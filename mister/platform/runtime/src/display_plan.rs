// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Main-compatible display-plan resolution for standalone device tools.

use crate::fpga::{Fpga, VideoInfo};
use mister_magik_core::display::{ResolvedDisplayPlan, RuntimeDisplayGeometry};
use std::io;

pub const RUNTIME_SETTINGS_ENV: &str = "MISTER_MAGIK_RUNTIME_SETTINGS_V1";
pub const RUNTIME_DISPLAY_ENV: &str = "MISTER_MAGIK_RUNTIME_DISPLAY_V1";

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
