// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Build immutable card artwork on the host; runtime draws its current text.
use mister_magik_framebuffer_scenes::launcher::{
    LauncherCardId, prepared_artwork::PreparedArtwork,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

fn style(key: &str, file: &str) -> (LauncherCardId, u16, u8) {
    use LauncherCardId::*;
    match key {
        "root:arcade" => (Arcade, 0xe1a5, 0),
        "root:consoles" => (Consoles, 0x2a7f, 1),
        "root:computers" => (Computers, 0xedc6, 2),
        "root:handhelds" => (Handhelds, 0x2df2, 3),
        "root:favourites" => (Favourites, 0xe12f, 4),
        "root:settings" => (Settings, 0x8b7f, 5),
        _ if key.starts_with("menu:computers:") || file.starts_with("computer-") => {
            (Computers, 0xedc6, 2)
        }
        _ if key.starts_with("menu:handhelds:") => (Handhelds, 0x2df2, 3),
        _ => (Consoles, 0x2a7f, 1),
    }
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let root = Path::new(
        args.first()
            .ok_or("usage: prepare_card_artwork DIRECTORY [--check]")?,
    );
    let check = args.get(1).is_some_and(|arg| arg == "--check");
    let mut index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("index.json"))?)?;
    let mut outputs = BTreeMap::new();
    for (key, card) in index["cards"].as_object_mut().ok_or("missing cards")? {
        let file = card["file"].as_str().ok_or("missing source file")?;
        if Path::new(file).file_name().and_then(|p| p.to_str()) != Some(file) {
            return Err("invalid source filename".into());
        }
        let (id, colour, tag) = style(key, file);
        let name = format!(
            "{}.{tag}-{colour:04x}.cardtex",
            file.trim_end_matches(".rgb888")
        );
        let bytes = match outputs.get(&name) {
            Some(bytes) => bytes,
            None => {
                let source = std::fs::read(root.join(file))?;
                if source.len() != 360 * 504 * 3
                    || card["sha256"].as_str() != Some(&digest(&source))
                {
                    return Err(format!("source integrity mismatch: {file}").into());
                }
                outputs
                    .entry(name.clone())
                    .or_insert_with(|| PreparedArtwork::encode(&source, id, colour))
            }
        };
        let prepared = serde_json::json!({"file":name,"bytes":bytes.len(),"sha256":digest(bytes)});
        if check && card["prepared"] != prepared {
            return Err(format!("prepared index is stale: {key}").into());
        }
        card["prepared"] = prepared;
    }
    let mut total = 0;
    for (name, bytes) in &outputs {
        if check {
            if std::fs::read(root.join(name))? != *bytes {
                return Err(format!("prepared artwork is stale: {name}").into());
            }
        } else {
            std::fs::write(root.join(name), bytes)?;
        }
        total += bytes.len();
    }
    if !check {
        std::fs::write(
            root.join("index.json"),
            serde_json::to_string_pretty(&index)? + "\n",
        )?;
    }
    println!(
        "{} prepared files; {} bytes; {}",
        outputs.len(),
        total,
        if check { "current" } else { "generated" }
    );
    Ok(())
}
