// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_fb::bitmap_font_resource::{
    generate_bacteria_12, generate_bacteria_12_native, generate_jersey_15, generate_jersey_25,
    generate_nocive_15, generate_spleen_5x8_doubled, generate_spleen_5x8_native,
    generate_spleen_6x12_doubled, generate_spleen_6x12_native, generate_terminus_8x14_bold,
    generate_terminus_8x14_native, generate_terminus_8x14_normal, generate_xerxes_10,
    generate_xerxes_10_crt240, generate_yesterday_10, generate_yesterday_10_crt240,
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
    let nocive_dir = private_asset_dir.join("fonts/nocive-15");
    let bacteria_dir = private_asset_dir.join("fonts/bacteria-12");
    std::fs::create_dir_all(&public_output_dir)?;
    std::fs::write(
        yesterday_dir.join("yesterday10-16px.mmbf"),
        generate_yesterday_10(&std::fs::read(yesterday_dir.join("Yesterday 10.ttf"))?)?,
    )?;
    std::fs::write(
        yesterday_dir.join("yesterday10-32px.mmbf"),
        generate_yesterday_10_crt240(&std::fs::read(yesterday_dir.join("Yesterday 10.ttf"))?)?,
    )?;
    std::fs::write(
        xerxes_dir.join("xerxes10-16px.mmbf"),
        generate_xerxes_10(&std::fs::read(xerxes_dir.join("Xerxes 10.ttf"))?)?,
    )?;
    std::fs::write(
        xerxes_dir.join("xerxes10-32px.mmbf"),
        generate_xerxes_10_crt240(&std::fs::read(xerxes_dir.join("Xerxes 10.ttf"))?)?,
    )?;
    std::fs::write(
        nocive_dir.join("nocive15-16px.mmbf"),
        generate_nocive_15(&std::fs::read(nocive_dir.join("Nocive 15.ttf"))?)?,
    )?;
    std::fs::write(
        bacteria_dir.join("bacteria12-32px.mmbf"),
        generate_bacteria_12(&std::fs::read(bacteria_dir.join("Bacteria 12.ttf"))?)?,
    )?;
    std::fs::write(
        bacteria_dir.join("bacteria12-16px.mmbf"),
        generate_bacteria_12_native(&std::fs::read(bacteria_dir.join("Bacteria 12.ttf"))?)?,
    )?;
    std::fs::write(
        public_output_dir.join("jersey15-27px.mmbf"),
        generate_jersey_15(&std::fs::read(
            public_font_dir.join("Jersey15-Regular.ttf"),
        )?)?,
    )?;
    std::fs::write(
        public_output_dir.join("jersey25-41px.mmbf"),
        generate_jersey_25(&std::fs::read(
            public_font_dir.join("Jersey25-Regular.ttf"),
        )?)?,
    )?;
    let terminus_dir = public_output_dir.join("terminus-8x14");
    std::fs::write(
        terminus_dir.join("terminus-8x14-normal-1x.mmbf"),
        generate_terminus_8x14_native(&std::fs::read_to_string(
            terminus_dir.join("ter-u14n.bdf"),
        )?)?,
    )?;
    std::fs::write(
        terminus_dir.join("terminus-8x14-normal-2x.mmbf"),
        generate_terminus_8x14_normal(&std::fs::read_to_string(
            terminus_dir.join("ter-u14n.bdf"),
        )?)?,
    )?;
    std::fs::write(
        terminus_dir.join("terminus-8x14-bold-2x.mmbf"),
        generate_terminus_8x14_bold(&std::fs::read_to_string(terminus_dir.join("ter-u14b.bdf"))?)?,
    )?;
    let spleen_dir = public_output_dir.join("spleen");
    let spleen_5x8 = std::fs::read_to_string(spleen_dir.join("spleen-5x8.bdf"))?;
    let spleen_6x12 = std::fs::read_to_string(spleen_dir.join("spleen-6x12.bdf"))?;
    std::fs::write(
        spleen_dir.join("spleen-5x8-1x.mmbf"),
        generate_spleen_5x8_native(&spleen_5x8)?,
    )?;
    std::fs::write(
        spleen_dir.join("spleen-5x8-2x.mmbf"),
        generate_spleen_5x8_doubled(&spleen_5x8)?,
    )?;
    std::fs::write(
        spleen_dir.join("spleen-6x12-1x.mmbf"),
        generate_spleen_6x12_native(&spleen_6x12)?,
    )?;
    std::fs::write(
        spleen_dir.join("spleen-6x12-2x.mmbf"),
        generate_spleen_6x12_doubled(&spleen_6x12)?,
    )?;
    Ok(())
}
