// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rerun-if-env-changed=SLINT_FONT_SIZES");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");
    println!("cargo:rustc-check-cfg=cfg(mister_bench_scenes)");

    // Embed both production UI fonts at every HDMI and CRT design size.
    let font_sizes = if let Ok(font_sizes) = std::env::var("SLINT_FONT_SIZES") {
        font_sizes
    } else {
        // SAFETY: Cargo runs this build script in its own process before the
        // script creates any threads.
        unsafe { std::env::set_var("SLINT_FONT_SIZES", "8,16,24,32") };
        "8,16,24,32".into()
    };

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
    if bench_scenes {
        println!("cargo:rustc-cfg=mister_bench_scenes");
        sources.push("../ui/experiments/effect_hud.slint");
    }

    let mut inputs = vec![
        "../ui/api.slint",
        "build.rs",
        "../ui/arcade_game.slint",
        "../ui/launcher_menu.slint",
        "../ui/components/launcher_chrome.slint",
        "../ui/components/overlays.slint",
        "../ui/components/progress.slint",
        "../ui/components/crt_ui.slint",
        "../ui/components/crt_combo_box.slint",
        "../ui/controller_panel.slint",
        "../ui/controller_setup.slint",
        "../ui/controller_test.slint",
        "../ui/bench/tear_pattern.slint",
        "../ui/bench/video_playback.slint",
        "../ui/launcher.slint",
        "../ui/mister_bridge.slint",
        "../ui/mister_window.slint",
        "../ui/variants/crt_overlays.slint",
        "../ui/variants/crt_setup.slint",
        "../ui/variants/crt_ui.slint",
        "../ui/variants/hdmi_overlays.slint",
        "../ui/variants/hdmi_setup.slint",
        "../ui/variants/hdmi_ui.slint",
        "../ui/views/crt/about.slint",
        "../ui/views/crt/arcade_list.slint",
        "../ui/views/crt/controller_panel.slint",
        "../ui/views/crt/home.slint",
        "../ui/views/crt/info.slint",
        "../ui/views/crt/licenses.slint",
        "../ui/views/crt/screensaver_settings.slint",
        "../ui/views/crt/settings.slint",
        "../ui/views/hdmi/about.slint",
        "../ui/views/hdmi/arcade_list.slint",
        "../ui/views/hdmi/home.slint",
        "../ui/views/hdmi/info.slint",
        "../ui/views/hdmi/licenses.slint",
        "../ui/views/hdmi/screensaver_settings.slint",
        "../ui/views/hdmi/settings.slint",
        "../ui/fonts/PressStart2P-Regular.ttf",
        "../ui/fonts/PressStart2P-Regular.ttf.license",
        "../ui/fonts/PressStart2P-PAL288-Regular.ttf",
        "../ui/fonts/PressStart2P-PAL288-Regular.ttf.license",
        "../ui/fonts/PressStart2P-PAL576-Regular.ttf",
        "../ui/fonts/PressStart2P-PAL576-Regular.ttf.license",
        "../ui/fonts/LilliputSteps.otf",
        "../ui/fonts/LilliputSteps.otf.license",
        "../ui/icons/settings.svg",
    ];
    if bench_scenes {
        inputs.push("../ui/experiments/effect_hud.slint");
    }

    for path in &inputs {
        println!("cargo:rerun-if-changed={path}");
    }

    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let fingerprint = fingerprint(&inputs, &font_sizes, launcher_only, bench_scenes);
    let fingerprint_path = out_dir.join("slint-inputs.fingerprint");
    if generated_outputs_exist(&out_dir, &sources)
        && std::fs::read_to_string(&fingerprint_path)
            .map(|old| old == fingerprint)
            .unwrap_or(false)
    {
        return;
    }

    for path in sources {
        let config = slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
        slint_build::compile_with_config(path, config)
            .unwrap_or_else(|e| panic!("Slint build failed for {path}: {e}"));
    }

    std::fs::write(fingerprint_path, fingerprint).expect("write Slint build fingerprint");
}

fn generated_outputs_exist(out_dir: &std::path::Path, sources: &[&str]) -> bool {
    sources.iter().all(|source| {
        let stem = std::path::Path::new(source)
            .file_stem()
            .expect("Slint source stem");
        out_dir.join(stem).with_extension("rs").is_file()
    })
}

fn fingerprint(
    inputs: &[&str],
    font_sizes: &str,
    launcher_only: bool,
    bench_scenes: bool,
) -> String {
    let mut state = 0xcbf29ce484222325u64;
    hash_bytes(&mut state, font_sizes.as_bytes());
    hash_bytes(&mut state, &[launcher_only as u8, bench_scenes as u8]);
    for path in inputs {
        hash_bytes(&mut state, path.as_bytes());
        hash_bytes(&mut state, b"\0");
        let bytes = std::fs::read(path)
            .unwrap_or_else(|e| panic!("failed to read Slint build input {path}: {e}"));
        hash_bytes(&mut state, &bytes);
    }
    format!("{state:016x}\n")
}

fn hash_bytes(state: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(0x100000001b3);
    }
}
