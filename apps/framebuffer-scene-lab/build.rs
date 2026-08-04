// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    println!("cargo:rerun-if-changed=src/card_flip_neon.c");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("arm") {
        return;
    }

    cc::Build::new()
        .file("src/card_flip_neon.c")
        .flag("-std=c11")
        .flag("-O3")
        .flag("-mtune=cortex-a9")
        .flag("-mfpu=neon-vfpv3")
        .flag("-mfloat-abi=hard")
        .flag("-ffp-contract=off")
        .compile("mister_magik_card_flip_neon");
}
