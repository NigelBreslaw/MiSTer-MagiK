// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_fb::bitmap_font_resource::{
    generate_jersey_25, generate_xerxes_10, generate_yesterday_10,
};
use std::path::PathBuf;

const USAGE: &str =
    "usage: generate-bitmap-fonts PUBLIC_FONT_DIR PRIVATE_ASSET_DIR PUBLIC_OUTPUT_DIR";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let public_font_dir = PathBuf::from(args.next().ok_or(USAGE)?);
    let private_asset_dir = PathBuf::from(args.next().ok_or(USAGE)?);
    let public_output_dir = PathBuf::from(args.next().ok_or(USAGE)?);
    if args.next().is_some() {
        return Err(USAGE.into());
    }

    let yesterday_dir = private_asset_dir.join("fonts/yesterday-10");
    let xerxes_dir = private_asset_dir.join("fonts/xerxes-10");
    std::fs::create_dir_all(&public_output_dir)?;
    std::fs::write(
        yesterday_dir.join("yesterday10-16px.mmbf"),
        generate_yesterday_10(&std::fs::read(yesterday_dir.join("Yesterday 10.ttf"))?)?,
    )?;
    std::fs::write(
        xerxes_dir.join("xerxes10-16px.mmbf"),
        generate_xerxes_10(&std::fs::read(xerxes_dir.join("Xerxes 10.ttf"))?)?,
    )?;
    std::fs::write(
        public_output_dir.join("jersey25-41px.mmbf"),
        generate_jersey_25(&std::fs::read(
            public_font_dir.join("Jersey25-Regular.ttf"),
        )?)?,
    )?;
    Ok(())
}
