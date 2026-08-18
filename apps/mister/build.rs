// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[path = "../../mister/platform/runtime/c_build_support.rs"]
mod c_build_support;

fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_VERSION");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_BUILD_TIME");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_SOURCE_REVISION");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_SOURCE_DIRTY");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src/particle_neon.c");
    println!("cargo:rerun-if-changed=src/arcade_list_neon.c");
    println!("cargo:rerun-if-changed=src/orientation_transition_neon.c");
    println!("cargo:rerun-if-changed=src/crt_backdrop_neon.c");
    println!("cargo:rerun-if-changed=../../mister/platform/runtime/c_build_support.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");
    println!("cargo:rustc-check-cfg=cfg(mister_bench_scenes)");
    println!("cargo:rustc-check-cfg=cfg(mister_experiments)");
    let build_number = git_commit_count();
    let version = release_version(&build_number);
    let source_revision = source_revision();
    let source_dirty = source_dirty();
    println!("cargo:rustc-env=MISTER_MAGIK_BUILD_NUMBER={build_number}");
    println!("cargo:rustc-env=MISTER_MAGIK_VERSION={version}");
    println!("cargo:rustc-env=MISTER_MAGIK_SOURCE_REVISION={source_revision}");
    println!("cargo:rustc-env=MISTER_MAGIK_SOURCE_DIRTY={source_dirty}");
    println!(
        "cargo:rustc-env=MISTER_MAGIK_BUILD_TIME={}",
        build_timestamp()
    );

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
            .file("src/arcade_list_neon.c")
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

fn release_version(build_number: &str) -> String {
    if let Ok(value) = std::env::var("MISTER_MAGIK_VERSION") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.into();
        }
    }
    format!("0.2.{build_number}")
}

fn git_commit_count() -> String {
    if let Ok(value) = std::env::var("MISTER_MAGIK_BUILD_NUMBER") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.into();
        }
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    command_stdout("git", &["-C", &manifest_dir, "rev-list", "--count", "HEAD"])
        .unwrap_or_else(|| "unknown".into())
}

fn build_timestamp() -> String {
    if let Ok(value) = std::env::var("MISTER_MAGIK_BUILD_TIME") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.into();
        }
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    command_output_text(
        "git",
        &[
            "-C",
            &manifest_dir,
            "show",
            "-s",
            "--format=%cd",
            "--date=format:%-d.%-m.%Y %H:%M",
            "HEAD",
        ],
    )
    .unwrap_or_else(|| "unknown".into())
}

fn source_revision() -> String {
    if let Some(value) = nonempty_env("MISTER_MAGIK_SOURCE_REVISION") {
        return value;
    }
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    command_stdout("git", &["-C", &manifest_dir, "rev-parse", "HEAD"])
        .unwrap_or_else(|| "unknown".into())
}

fn source_dirty() -> String {
    if let Some(value) = nonempty_env("MISTER_MAGIK_SOURCE_DIRTY") {
        return value;
    }
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    command_stdout(
        "git",
        &[
            "-C",
            &manifest_dir,
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
    )
    .map_or_else(
        || "unknown".into(),
        |output| {
            if output.is_empty() {
                "0".into()
            } else {
                "1".into()
            }
        },
    )
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn command_stdout(command: &str, args: &[&str]) -> Option<String> {
    command_output_text(command, args).filter(|text| !text.is_empty())
}

fn command_output_text(command: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.trim().into())
}
