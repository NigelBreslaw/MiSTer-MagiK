#![allow(clippy::all, unused_imports)]
// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(mister_bench_scenes)]
pub mod effect_hud {
    include!(concat!(env!("OUT_DIR"), "/effect_hud.rs"));
}

#[cfg(not(mister_ui_scope_launcher))]
pub mod video_playback {
    include!(concat!(env!("OUT_DIR"), "/video_playback.rs"));
}

#[cfg(not(mister_ui_scope_launcher))]
pub mod controller {
    include!(concat!(env!("OUT_DIR"), "/controller_test.rs"));
}

pub mod launcher {
    include!(concat!(env!("OUT_DIR"), "/launcher.rs"));
}

#[cfg(not(mister_ui_scope_launcher))]
pub mod tear_pattern {
    include!(concat!(env!("OUT_DIR"), "/tear_pattern.rs"));
}
