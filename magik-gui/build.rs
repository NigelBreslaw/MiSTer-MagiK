fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");
    println!("cargo:rustc-check-cfg=cfg(mister_bench_scenes)");
    println!("cargo:rustc-check-cfg=cfg(mister_experiments)");

    let scope = std::env::var("MISTER_UI_BUILD_SCOPE").unwrap_or_else(|_| "all".into());
    let launcher_only = match scope.as_str() {
        "" | "all" => false,
        "launcher" | "arcade" => true,
        other => panic!("unknown MISTER_UI_BUILD_SCOPE={other:?}; use all|launcher|arcade"),
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

    let video = std::env::var_os("CARGO_FEATURE_VIDEO").is_some();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if video && target_arch == "arm" {
        cc::Build::new()
            .file("src/video_i420_rgb565_neon.c")
            .flag("-mfpu=neon")
            .warnings(false)
            .compile("mister_video_i420_rgb565_neon");
        println!("cargo:rerun-if-changed=src/video_i420_rgb565_neon.c");
    }
}
