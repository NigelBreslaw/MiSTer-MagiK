// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[path = "../../mister/platform/runtime/c_build_support.rs"]
mod c_build_support;

fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src/particle_neon.c");
    println!("cargo:rerun-if-changed=src/orientation_transition_neon.c");
    println!("cargo:rerun-if-changed=src/crt_backdrop_neon.c");
    println!("cargo:rerun-if-changed=../../mister/platform/runtime/c_build_support.rs");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");
    println!("cargo:rustc-check-cfg=cfg(mister_bench_scenes)");
    println!("cargo:rustc-check-cfg=cfg(mister_experiments)");
    let scope = std::env::var("MISTER_UI_BUILD_SCOPE").unwrap_or_else(|_| "all".into());
    let launcher_only = match scope.as_str() {
        "" | "all" => false,
        "launcher" | "arcade" | "production" => true,
        other => {
            panic!("unknown MISTER_UI_BUILD_SCOPE={other:?}; use all|launcher|arcade|production")
        }
    };
    if launcher_only {
        println!("cargo:rustc-cfg=mister_ui_scope_launcher");
    }
    let bench_scenes = std::env::var_os("CARGO_FEATURE_BENCH_SCENES").is_some();
    if bench_scenes {
        println!("cargo:rustc-cfg=mister_bench_scenes");
    }
    let experiments = std::env::var_os("CARGO_FEATURE_EXPERIMENTS").is_some();
    if experiments {
        println!("cargo:rustc-cfg=mister_experiments");
    }
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("arm") {
        let mut particle_neon = c_build();
        particle_neon
            .file("src/particle_neon.c")
            .file("src/orientation_transition_neon.c")
            .file("src/crt_backdrop_neon.c")
            .flag("-std=c11")
            .flag("-O3")
            .flag("-mtune=cortex-a9")
            .flag("-mfpu=neon-vfpv3")
            .flag("-mfloat-abi=hard")
            .flag("-ffp-contract=off");
        particle_neon.compile("mister_magik_scanline_neon");
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
