// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");
    println!("cargo:rustc-check-cfg=cfg(mister_bench_scenes)");

    let bench_scenes = std::env::var_os("CARGO_FEATURE_BENCH_SCENES").is_some();
    let scope = std::env::var("MISTER_UI_BUILD_SCOPE").unwrap_or_else(|_| "production".into());
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

    let mut sources = vec!["../ui/launcher.slint"];
    if !launcher_only {
        sources.extend([
            "../ui/controller_test.slint",
            "../ui/bench/tear_pattern.slint",
            "../ui/bench/video_playback.slint",
        ]);
    }
    if bench_scenes {
        println!("cargo:rustc-cfg=mister_bench_scenes");
        sources.push("../ui/experiments/effect_hud.slint");
    }

    for path in sources {
        // Text uses runtime-registered bitmap fonts and the production UI has
        // no file-backed images, so no software-renderer glyph atlas is needed.
        let config = slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
        slint_build::compile_with_config(path, config)
            .unwrap_or_else(|e| panic!("Slint build failed for {path}: {e}"));
    }
}
