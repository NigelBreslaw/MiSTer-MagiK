fn main() {
    compile_ui();
}

#[cfg(feature = "ui")]
fn compile_ui() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");

    // Press Start 2P at all app design sizes used by the 960×540 UI.
    if std::env::var("SLINT_FONT_SIZES").is_err() {
        std::env::set_var("SLINT_FONT_SIZES", "8,16,24,32,48");
    }

    let scope = std::env::var("MISTER_UI_BUILD_SCOPE").unwrap_or_else(|_| "all".into());
    let launcher_only = match scope.as_str() {
        "" | "all" => false,
        "launcher" => true,
        other => panic!("unknown MISTER_UI_BUILD_SCOPE={other:?}; use all|launcher"),
    };
    if launcher_only {
        println!("cargo:rustc-cfg=mister_ui_scope_launcher");
    }

    let mut sources = vec![
        "ui/app.slint",
        "ui/controller_test.slint",
        "ui/debug.slint",
        "ui/launcher.slint",
    ];
    if !launcher_only {
        sources.extend([
            "ui/bench/full_motion.slint",
            "ui/bench/static_ui.slint",
            "ui/bench/local_motion.slint",
            "ui/bench/text_heavy.slint",
            "ui/bench/solid_fill.slint",
            "ui/bench/list_scroll.slint",
            "ui/bench/console_scroll.slint",
            "ui/bench/dirty_band.slint",
        ]);
        if std::env::var_os("CARGO_FEATURE_VIDEO").is_some() {
            sources.push("ui/bench/video_playback.slint");
        }
    }

    for path in sources {
        let config = slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
        slint_build::compile_with_config(path, config)
            .unwrap_or_else(|e| panic!("Slint build failed for {path}: {e}"));
    }
}

#[cfg(not(feature = "ui"))]
fn compile_ui() {}
