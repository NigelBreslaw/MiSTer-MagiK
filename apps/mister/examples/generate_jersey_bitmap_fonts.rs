// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_fb::bitmap_font_resource::{generate_jersey_10, generate_jersey_25};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let font_dir = PathBuf::from(
        args.next()
            .ok_or("usage: generate-jersey-bitmap-fonts FONT_DIR OUTPUT_DIR")?,
    );
    let output_dir = PathBuf::from(
        args.next()
            .ok_or("usage: generate-jersey-bitmap-fonts FONT_DIR OUTPUT_DIR")?,
    );
    if args.next().is_some() {
        return Err("usage: generate-jersey-bitmap-fonts FONT_DIR OUTPUT_DIR".into());
    }
    std::fs::create_dir_all(&output_dir)?;
    let jersey_10 = std::fs::read(font_dir.join("Jersey10-Regular.ttf"))?;
    let jersey_25 = std::fs::read(font_dir.join("Jersey25-Regular.ttf"))?;
    std::fs::write(
        output_dir.join("jersey10-22px.mmbf"),
        generate_jersey_10(&jersey_10)?,
    )?;
    std::fs::write(
        output_dir.join("jersey25-41px.mmbf"),
        generate_jersey_25(&jersey_25)?,
    )?;
    Ok(())
}
