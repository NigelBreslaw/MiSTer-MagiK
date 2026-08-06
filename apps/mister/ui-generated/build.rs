// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rerun-if-env-changed=SLINT_FONT_SIZES");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");
    println!("cargo:rustc-check-cfg=cfg(mister_bench_scenes)");

    // Embed the production UI font at every HDMI and CRT design size.
    if std::env::var_os("SLINT_FONT_SIZES").is_none() {
        // SAFETY: Cargo runs this build script in its own process before the
        // script creates any threads.
        unsafe { std::env::set_var("SLINT_FONT_SIZES", "8,16,24,32") };
    }

    let bench_scenes = std::env::var_os("CARGO_FEATURE_BENCH_SCENES").is_some();
    let scope = std::env::var("MISTER_UI_BUILD_SCOPE").unwrap_or_else(|_| "all".into());
    let launcher_only = match scope.as_str() {
        "" | "all" => false,
        "launcher" | "arcade" => true,
        other => panic!("unknown MISTER_UI_BUILD_SCOPE={other:?}; use all|launcher|arcade"),
    };
    if launcher_only {
        println!("cargo:rustc-cfg=mister_ui_scope_launcher");
    }

    let mut sources = vec![
        "../ui/controller_test.slint",
        "../ui/launcher.slint",
        "../ui/bench/tear_pattern.slint",
        "../ui/bench/video_playback.slint",
    ];
    if !launcher_only {
        sources.extend([
            "../ui/mockups/crt_arcade_list_mockup.slint",
            "../ui/mockups/crt_launcher_mockup.slint",
            "../ui/mockups/crt_resolution_combo_mockup.slint",
            "../ui/mockups/crt_settings_mockup.slint",
            "../ui/mockups/crt_systems_list_mockup.slint",
        ]);
    }
    if bench_scenes {
        println!("cargo:rustc-cfg=mister_bench_scenes");
        sources.push("../ui/experiments/effect_hud.slint");
    }

    for path in sources {
        let config = slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
        slint_build::compile_with_config(path, config)
            .unwrap_or_else(|e| panic!("Slint build failed for {path}: {e}"));
    }
}
