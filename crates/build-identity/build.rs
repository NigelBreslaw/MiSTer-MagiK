// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;
use std::process::Command;

fn main() {
    for name in [
        "MISTER_MAGIK_BUILD_NUMBER",
        "MISTER_MAGIK_VERSION",
        "MISTER_MAGIK_BUILD_TIME",
        "MISTER_MAGIK_SOURCE_REVISION",
        "MISTER_MAGIK_SOURCE_DIRTY",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    let build_number = env_or("MISTER_MAGIK_BUILD_NUMBER", || {
        git(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "unknown".into())
    });
    let version = env_or("MISTER_MAGIK_VERSION", || format!("0.2.{build_number}"));
    let build_time = env_or("MISTER_MAGIK_BUILD_TIME", || {
        git_with_args(&[
            "show",
            "-s",
            "--format=%cd",
            "--date=format:%-d.%-m.%Y %H:%M",
            "HEAD",
        ])
        .unwrap_or_else(|| "unknown".into())
    });
    let source_revision = env_or("MISTER_MAGIK_SOURCE_REVISION", || {
        git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into())
    });
    let source_dirty = env_or("MISTER_MAGIK_SOURCE_DIRTY", || {
        let dirty = Command::new("git")
            .current_dir(repository_root())
            .args(["status", "--porcelain", "--untracked-files=all"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| !output.stdout.is_empty());
        if dirty { "1".into() } else { "0".into() }
    });

    println!("cargo:rustc-env=MISTER_MAGIK_BUILD_NUMBER={build_number}");
    println!("cargo:rustc-env=MISTER_MAGIK_VERSION={version}");
    println!("cargo:rustc-env=MISTER_MAGIK_BUILD_TIME={build_time}");
    println!("cargo:rustc-env=MISTER_MAGIK_SOURCE_REVISION={source_revision}");
    println!("cargo:rustc-env=MISTER_MAGIK_SOURCE_DIRTY={source_dirty}");
}

fn repository_root() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default()).join("../..")
}

fn env_or(name: &str, fallback: impl FnOnce() -> String) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(fallback)
}

fn git(args: &[&str]) -> Option<String> {
    git_with_args(args)
}

fn git_with_args(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repository_root())
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
