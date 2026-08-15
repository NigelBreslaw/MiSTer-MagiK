// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! MiSTer-specific adapters kept outside the application and portable domain.

#[macro_export]
macro_rules! ui_errln {
    ($($arg:tt)*) => {{ eprintln!($($arg)*); }};
}

#[macro_export]
macro_rules! ui_logln {
    ($($arg:tt)*) => {{ println!($($arg)*); }};
}

pub mod boot_analytics;
#[cfg(feature = "app-runtime")]
pub mod direct_reset_fault;
pub mod display_plan;
pub mod display_resolution;
pub mod fpga;
pub mod framebuffer;
#[cfg(all(feature = "framebuffer-lab", target_os = "linux"))]
pub mod lab_input;
pub mod latch_readiness;
#[cfg(feature = "app-runtime")]
pub mod main_command;
#[cfg(feature = "app-runtime")]
pub mod runtime_state;
#[cfg(feature = "app-runtime")]
pub mod runtime_status;
#[cfg(feature = "app-runtime")]
pub mod settings;
pub mod vt;
