// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    println!("cargo:rerun-if-changed=ui/main.slint");
    println!("cargo:rerun-if-changed=ui/api.slint");
    println!("cargo:rerun-if-changed=ui/components/chrome.slint");
    println!("cargo:rerun-if-changed=ui/views/analytics.slint");
    println!("cargo:rerun-if-changed=ui/views/debug.slint");
    println!("cargo:rerun-if-changed=ui/views/sd_card.slint");
    if std::env::var_os("CARGO_FEATURE_COMPILED_UI").is_some() {
        slint_build::compile("ui/main.slint").expect("compile Slint UI");
    }
}
