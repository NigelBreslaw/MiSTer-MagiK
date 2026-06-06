//! Host-testable logic for the MiSTer frontend.
//!
//! The framebuffer, FPGA, Linux input, and Slint renderer stay in the binary
//! target behind the `ui` feature. This library keeps pure logic available for
//! fast macOS host tests without compiling Slint/AppKit.

pub mod arcade_catalog;
pub mod controller_db;
pub mod framebuffer_copy;
pub mod input_info;
pub mod input_repeat;
pub mod library_bench;
