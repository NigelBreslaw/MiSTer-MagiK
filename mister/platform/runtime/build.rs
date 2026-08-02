// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

fn main() {
    println!("cargo:rustc-check-cfg=cfg(mister_arm_scalar_decimator)");
    println!("cargo:rerun-if-changed=src/framebuffer/downsample_scalar.c");
    #[cfg(feature = "ui")]
    {
        if std::env::var("TARGET").as_deref() == Ok("armv7-unknown-linux-gnueabihf") {
            println!("cargo:rustc-cfg=mister_arm_scalar_decimator");
            cc::Build::new()
                .file("src/framebuffer/downsample_scalar.c")
                .flag("-mtune=cortex-a9")
                .flag("-fno-tree-vectorize")
                .warnings_into_errors(true)
                .compile("mister_magik_downsample_scalar");
        }
    }
}
