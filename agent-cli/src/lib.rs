// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![recursion_limit = "256"]

mod archive;
pub mod benchmark;
pub mod cli;
pub mod commands;
pub mod device;
pub mod doctor;
pub mod error;
pub mod evidence;
pub mod git;
mod host;
pub mod model;
pub mod platform_bundle;
pub mod platform_manifest;
pub mod process;
pub mod progress;
pub mod redact;
pub mod request;
pub mod return_qualification;
pub mod transport;
pub mod workflow;

pub use host::NativeDevice;
