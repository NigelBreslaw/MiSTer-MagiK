//! Host-testable logic for the MiSTer frontend.
//!
//! The framebuffer, FPGA, Linux input, and Slint renderer stay in the binary
//! target behind the `ui` feature. This library keeps pure logic available for
//! fast macOS host tests without compiling Slint/AppKit.

pub mod arcade_button_overrides;
pub mod boot_analytics;
pub mod command_args;
pub mod controller_db;
pub mod crash_report;
mod fallible_log;
pub mod framebuffer;
pub mod input_info;
pub mod input_repeat;
pub mod input_state;
pub mod launch_preparation;
pub mod launcher;
pub mod media_update;
pub mod raw565;
pub mod runtime_status;
pub mod settings;
pub mod setup_nav;
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
