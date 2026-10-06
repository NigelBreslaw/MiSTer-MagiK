// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral launcher runtime decisions shared by the MiSTer and macOS UI.

pub mod catalog;
pub mod composition;
pub mod full_screen_transition;
pub mod input_router;
pub mod lifecycle;
pub mod media;
pub mod navigation_transition;
pub mod orientation_transition;
pub mod orientation_transition_bench;
pub mod settings;
pub mod settings_navigation_bench;
pub mod startup_intro;
pub mod transition_lifecycle;
pub mod transition_plan;
#[cfg(test)]
mod transition_scenarios;
pub mod transition_spec;
#[cfg(test)]
mod walk_rng;
