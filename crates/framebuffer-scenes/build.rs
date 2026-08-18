// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[path = "../../mister/platform/runtime/c_build_support.rs"]
mod c_build_support;

fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_RGB565_NEON");
    println!("cargo:rerun-if-changed=src/rgb565_neon.c");
    println!("cargo:rerun-if-changed=../../mister/platform/runtime/c_build_support.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("arm") {
        return;
    }
    let mut build = cc::Build::new();
    build
        .inherit_rustflags(false)
        .file("src/rgb565_neon.c")
        .flag("-std=c11")
        .flag("-O3")
        .flag("-mtune=cortex-a9")
        .flag("-mfpu=neon-vfpv3")
        .flag("-mfloat-abi=hard")
        .flag("-ffp-contract=off");
    if c_build_support::force_frame_pointers_requested() {
        build.force_frame_pointer(true);
    }
    build.compile("mister_magik_rgb565_neon");
}
