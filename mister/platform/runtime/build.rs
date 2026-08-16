// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[path = "c_build_support.rs"]
mod c_build_support;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(mister_arm_neon_decimator)");
    println!("cargo:rerun-if-changed=src/framebuffer/downsample_neon.c");
    println!("cargo:rerun-if-changed=c_build_support.rs");
    #[cfg(feature = "ui")]
    {
        if std::env::var("TARGET").as_deref() == Ok("armv7-unknown-linux-gnueabihf") {
            println!("cargo:rustc-cfg=mister_arm_neon_decimator");
            c_build()
                .file("src/framebuffer/downsample_neon.c")
                .flag("-std=c11")
                .flag("-O3")
                .flag("-mtune=cortex-a9")
                .flag("-mfpu=neon-vfpv3")
                .flag("-mfloat-abi=hard")
                .warnings_into_errors(true)
                .compile("mister_magik_downsample_neon");
        }
    }
}

fn c_build() -> cc::Build {
    let mut build = cc::Build::new();
    build.inherit_rustflags(false);
    if c_build_support::force_frame_pointers_requested() {
        build.force_frame_pointer(true);
    }
    build
}
