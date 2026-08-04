// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable MiSTer MagiK domain state.
//!
//! This crate deliberately has no GUI, operating-system, filesystem, FPGA, or
//! process-control dependencies. Applications provide those capabilities by
//! implementing [`platform::MagikPlatform`].

pub mod display;
pub mod input_info;
pub mod input_repeat;
pub mod input_state;
pub mod platform;
