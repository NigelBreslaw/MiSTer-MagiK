// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    println!("cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_VERSION");
    println!("cargo:rerun-if-env-changed=MISTER_MAGIK_BUILD_TIME");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
    println!("cargo:rustc-check-cfg=cfg(mister_ui_scope_launcher)");
    println!("cargo:rustc-check-cfg=cfg(mister_bench_scenes)");
    println!("cargo:rustc-check-cfg=cfg(mister_experiments)");
    println!("cargo:rustc-check-cfg=cfg(mister_arm_scalar_decimator)");
    let build_number = git_commit_count();
    let version = release_version(&build_number);
    println!("cargo:rustc-env=MISTER_MAGIK_BUILD_NUMBER={build_number}");
    println!("cargo:rustc-env=MISTER_MAGIK_VERSION={version}");
    println!(
        "cargo:rustc-env=MISTER_MAGIK_BUILD_TIME={}",
        build_timestamp()
    );

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

    let target = std::env::var("TARGET").unwrap_or_default();
    if target == "armv7-unknown-linux-gnueabihf" {
        println!("cargo:rustc-cfg=mister_arm_scalar_decimator");
        println!("cargo:rerun-if-changed=src/framebuffer/downsample_scalar.c");
        cc::Build::new()
            .file("src/framebuffer/downsample_scalar.c")
            .flag("-mtune=cortex-a9")
            .flag("-fno-tree-vectorize")
            .warnings_into_errors(true)
            .compile("mister_magik_downsample_scalar");
    }
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
    command_stdout(
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

fn command_stdout(command: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.into())
    }
}
