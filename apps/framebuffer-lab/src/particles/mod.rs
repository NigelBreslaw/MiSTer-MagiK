// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic particle renderer and hot-reloadable recipe families.

pub mod fireworks;
pub mod fireworks_v2;
pub mod form;
pub mod live_reload;
pub mod material;
mod recipes;
pub mod showcase;

pub use recipes::ParticleRecipeFamily;
