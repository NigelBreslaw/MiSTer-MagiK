// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native MiSTer MagiK framebuffer process bootstrap.

#[global_allocator]
static GLOBAL_ALLOCATOR: mister_magik_fb::allocation_metrics::TrackingAllocator =
    mister_magik_fb::allocation_metrics::TrackingAllocator;

fn main() {
    // SAFETY: this is the first operation in main, before hooks, UI state, or
    // worker threads exist.
    unsafe { mister_magik_catalog::device_layout::initialize_process_env() };
    mister_magik_fb::app_entry::run();
}
