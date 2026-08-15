// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable MiSTer MagiK domain state.
//!
//! This crate deliberately has no GUI, operating-system, filesystem, FPGA, or
//! process-control dependencies. Applications provide the narrow capabilities
//! declared by the portable domain modules.

pub mod display;
pub mod input_event;
pub mod input_info;
pub mod input_repeat;
pub mod input_state;
pub mod launcher_effects;
