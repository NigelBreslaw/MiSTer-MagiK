// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![recursion_limit = "256"]

pub mod alpha;
pub mod architecture;
mod archive;
pub mod benchmark;
pub mod build;
pub mod capture;
pub mod checks;
pub mod ci;
pub mod clean;
pub mod cli;
pub mod commands;
pub mod compile_time;
pub mod components;
pub mod delivery;
pub mod dependencies;
pub mod deploy;
pub mod device;
pub mod diagnose;
pub mod doctor;
pub mod error;
pub mod evidence;
pub mod executor;
pub mod fpga;
pub mod game_databases;
pub mod git;
pub mod hooks;
mod host;
pub mod lab_build;
pub mod live_particles;
pub mod local_main_delivery;
pub mod model;
pub mod planner;
pub mod platform_bundle;
pub mod platform_ci;
pub mod platform_manifest;
mod platform_stage;
pub mod process;
pub mod progress;
pub mod redact;
pub mod release;
pub mod request;
pub mod return_qualification;
pub mod scope;
mod shell;
pub mod startup_particles;
pub mod transport;
pub mod workflow;

pub use host::NativeDevice;
