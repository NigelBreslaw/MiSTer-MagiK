// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-testable logic for the MiSTer frontend.
//!
//! The framebuffer, FPGA, Linux input, and Slint renderer stay in the binary
//! target behind the `ui` feature. This library keeps pure logic available for
//! fast macOS host tests without compiling Slint/AppKit.

extern crate self as mister_magik_fb;

pub mod arcade_button_overrides;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
#[allow(dead_code)]
mod arcade_list_renderer;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
#[allow(dead_code)]
#[doc(hidden)]
pub mod artifact_publish;
pub use mister_magik_mister_runtime::boot_analytics;
pub mod bitmap_font_resource;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
#[allow(dead_code)]
mod bitmap_text;
pub mod build_identity;
pub mod catalog_failure_report;
pub mod catalog_progress_report;
pub mod command_args;
pub mod controller_db;
pub mod crash_report;
pub mod diagnostic_identity;
#[doc(hidden)]
pub mod fallible_log;
pub use mister_magik_core::{input_event, input_info, input_repeat, input_state};
pub use mister_magik_mister_runtime::framebuffer;
pub use mister_magik_mister_runtime::latch_readiness;
pub mod latch_failure_report;
pub mod launch_preparation;
pub mod launcher;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub mod launcher_presentation;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub mod launcher_runtime;
pub mod launcher_taxonomy;
pub mod licenses;
#[cfg(all(feature = "ui-preview", target_os = "macos"))]
pub mod macos_preview_content;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
mod media_http;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
mod media_pack_save;
pub mod media_update;
pub mod particle_engine;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub mod particle_renderer;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub mod preview_transition;
pub mod process_config;
#[cfg(all(feature = "ui-preview", target_os = "macos"))]
#[path = "ui_runner/launcher_screensaver.rs"]
pub mod production_launcher_screensaver;
pub mod raw565;
pub mod return_catalog_capsule;
pub use mister_magik_mister_runtime::runtime_status;
pub use mister_magik_mister_runtime::settings;
pub mod setup_nav;
pub mod snes_artwork;
pub mod spring_animation;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub mod startup_particles;
#[cfg(test)]
mod test_support;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
#[allow(dead_code)]
pub mod ui_display;
#[cfg(all(feature = "ui-preview", target_os = "macos"))]
pub mod ui_preview_fixtures;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub mod visual_composition;
#[cfg(any(feature = "ui", feature = "ui-preview"))]
pub mod visual_platform;
#[cfg(mister_experiments)]
pub mod experiments {
    pub mod effects;
}
#[cfg(test)]
mod video_i420;

pub use mister_magik_catalog::{
    arcade_catalog, library_bench, library_db, media_identity, preview_worker,
};
