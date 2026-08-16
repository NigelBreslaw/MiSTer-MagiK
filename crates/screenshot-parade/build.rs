// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[path = "../../mister/platform/runtime/c_build_support.rs"]
mod c_build_support;

use std::fmt::Write;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/screenshot_phase_neon.c");
    println!("cargo:rerun-if-changed=../../mister/platform/runtime/c_build_support.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("arm") {
        return;
    }

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let reciprocal_header = out_dir.join("screenshot_phase_reciprocals.h");
    let mut header = String::from(
        "#include <stdint.h>\nstatic const uint32_t SCREENSHOT_PHASE_RECIPROCALS[65536] = {\n",
    );
    for divisor in 0_u32..=u32::from(u16::MAX) {
        let reciprocal = if divisor < 2 {
            0
        } else {
            ((1_u64 << 32) / u64::from(divisor)) as u32
        };
        writeln!(header, "{reciprocal},").expect("write reciprocal header");
    }
    header.push_str("};\n");
    std::fs::write(&reciprocal_header, header).expect("write reciprocal header");

    c_build()
        .file("src/screenshot_phase_neon.c")
        .include(out_dir)
        .flag("-std=c11")
        .flag("-O3")
        .flag("-mtune=cortex-a9")
        .flag("-mfpu=neon-vfpv3")
        .flag("-mfloat-abi=hard")
        .flag("-ffp-contract=off")
        .compile("mister_magik_screenshot_phase_neon");
}

fn c_build() -> cc::Build {
    let mut build = cc::Build::new();
    build.inherit_rustflags(false);
    if c_build_support::force_frame_pointers_requested() {
        build.force_frame_pointer(true);
    }
    build
}
