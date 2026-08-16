// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[path = "../../mister/platform/runtime/c_build_support.rs"]
mod c_build_support;

fn main() {
    println!("cargo:rerun-if-changed=src/particle_neon.c");
    println!("cargo:rerun-if-changed=../../mister/platform/runtime/c_build_support.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("arm") {
        return;
    }
    c_build()
        .file("src/particle_neon.c")
        .define("MISTER_MAGIK_EXPERIMENTS", None)
        .flag("-std=c11")
        .flag("-O3")
        .flag("-mtune=cortex-a9")
        .flag("-mfpu=neon-vfpv3")
        .flag("-mfloat-abi=hard")
        .flag("-ffp-contract=off")
        .compile("mister_magik_particle_lab_neon");
}

fn c_build() -> cc::Build {
    let mut build = cc::Build::new();
    build.inherit_rustflags(false);
    if c_build_support::force_frame_pointers_requested() {
        build.force_frame_pointer(true);
    }
    build
}
