// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Experiment-only software-rendered effect scenes.

use super::super::*;

mod camera_effects_loop;
mod effect_loop_support;
mod raster_effects_loop;
mod sprite_effects_loop;
mod text_effects_loop;
mod transition_effects_loop;

pub(in crate::ui_runner) use camera_effects_loop::{print_camera_effects, run_camera_effects_loop};
pub(in crate::ui_runner) use raster_effects_loop::{print_raster_effects, run_raster_effects_loop};
pub(in crate::ui_runner) use sprite_effects_loop::{print_sprite_effects, run_sprite_effects_loop};
pub(in crate::ui_runner) use text_effects_loop::{print_text_effects, run_text_effects_loop};
pub(in crate::ui_runner) use transition_effects_loop::{
    print_transition_effects, run_transition_effects_loop,
};
