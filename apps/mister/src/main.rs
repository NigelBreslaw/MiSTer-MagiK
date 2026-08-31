// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native MiSTer MagiK framebuffer process bootstrap.

#[global_allocator]
static GLOBAL_ALLOCATOR: mister_magik_fb::allocation_metrics::TrackingAllocator =
    mister_magik_fb::allocation_metrics::TrackingAllocator;

fn main() {
    // Hotpath remains compiled but dormant until an attended benchmark enables
    // its loopback metrics and MCP servers.
    #[cfg(feature = "hotpath")]
    let _hotpath_guard = std::env::var_os("MISTER_HOTPATH")
        .map(|_| hotpath::HotpathGuardBuilder::new("mister_magik_fb::main").build());
    // Keep executable bootstrap thin; lifecycle ownership lives in app_entry.
    mister_magik_fb::app_entry::run();
}
