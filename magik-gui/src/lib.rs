//! Host-testable logic for the MiSTer frontend.
//!
//! The framebuffer, FPGA, Linux input, and Slint renderer stay in the binary
//! target behind the `ui` feature. This library keeps pure logic available for
//! fast macOS host tests without compiling Slint/AppKit.

pub mod boot_analytics;
pub mod camera_effects;
pub mod command_args;
pub mod controller_db;
pub mod crash_report;
pub mod effects;
pub mod framebuffer;
pub mod input_info;
pub mod input_repeat;
pub mod input_state;
pub mod launch_preparation;
pub mod launcher;
pub mod media_update;
pub mod raster_effects;
pub mod raw565;
pub mod runtime_status;
pub mod setup_nav;
pub mod sprite_effects;
pub mod text_effects;
pub mod transition_effects;
#[cfg(test)]
mod video_i420;

pub use mister_magik_catalog::{
    arcade_catalog, library_bench, library_db, media_identity, preview_worker,
};
