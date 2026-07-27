// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-testable logic for the MiSTer frontend.
//!
//! The framebuffer, FPGA, Linux input, and Slint renderer stay in the binary
//! target behind the `ui` feature. This library keeps pure logic available for
//! fast macOS host tests without compiling Slint/AppKit.

pub mod arcade_button_overrides;
pub use mister_magik_mister_runtime::boot_analytics;
pub mod catalog_failure_report;
pub mod command_args;
pub mod controller_db;
pub mod crash_report;
mod fallible_log;
pub use mister_magik_core::{input_info, input_repeat, input_state};
pub use mister_magik_mister_runtime::framebuffer;
pub use mister_magik_mister_runtime::latch_readiness;
pub mod latch_failure_report;
pub mod launch_preparation;
pub mod launcher;
pub mod launcher_taxonomy;
pub mod licenses;
pub mod media_update;
pub mod particle_engine;
pub mod raw565;
pub mod return_catalog_capsule;
pub use mister_magik_mister_runtime::runtime_status;
pub use mister_magik_mister_runtime::settings;
pub mod setup_nav;
pub mod spring_animation;
#[cfg(test)]
mod test_support;
#[cfg(mister_experiments)]
pub mod experiments {
    pub mod effects;
}
#[cfg(test)]
mod video_i420;

pub use mister_magik_catalog::{
    arcade_catalog, library_bench, library_db, media_identity, preview_worker,
};
