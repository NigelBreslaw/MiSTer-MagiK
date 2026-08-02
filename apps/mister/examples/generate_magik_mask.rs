// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub use mister_magik_fb::framebuffer;

#[path = "../src/bitmap_text.rs"]
mod bitmap_text;

use bitmap_text::{ConsoleFont, ConsoleTypeface};
use std::io::Write;
use std::path::PathBuf;

const MAGIC: &[u8; 8] = b"MAGIKMSK";
const VERSION: u16 = 1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: generate_magik_mask OUTPUT")?;
    let mut font = ConsoleFont::new_with_typeface(128.0, ConsoleTypeface::PressStart2P);
    let mask = font
        .rasterize_alpha_mask("MagiK")
        .ok_or("Press Start 2P produced no MagiK alpha mask")?;
    let width = u16::try_from(mask.width)?;
    let height = u16::try_from(mask.height)?;
    let stride = u16::try_from(mask.stride)?;
    let mut file = std::fs::File::create(output)?;
    file.write_all(MAGIC)?;
    file.write_all(&VERSION.to_le_bytes())?;
    file.write_all(&width.to_le_bytes())?;
    file.write_all(&height.to_le_bytes())?;
    file.write_all(&stride.to_le_bytes())?;
    file.write_all(&mask.alpha)?;
    Ok(())
}
