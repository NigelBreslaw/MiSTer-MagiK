// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Build immutable card artwork on the host; runtime draws its current text.
use mister_magik_framebuffer_scenes::launcher::{
    LauncherCardId, LauncherCardStyle, prepared_artwork::PreparedArtwork,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

fn style(
    key: &str,
    sections: &BTreeMap<String, String>,
) -> Result<(LauncherCardId, u16, u8), String> {
    use LauncherCardId::*;
    let style = if let Some(root) = key.strip_prefix("root:") {
        LauncherCardStyle::root(match root {
            "arcade" => Arcade,
            "consoles" => Consoles,
            "computers" => Computers,
            "handhelds" => Handhelds,
            "favourites" => Favourites,
            "settings" => Settings,
            _ => return Err(format!("unknown root artwork key: {key}")),
        })
    } else {
        let section = if key.starts_with("menu:") {
            key.split(':').nth(1).ok_or("missing menu section")?
        } else {
            sections
                .get(key)
                .ok_or_else(|| format!("unknown taxonomy artwork key: {key}"))?
        };
        LauncherCardStyle::section(section)
    };
    let tag = match style.id {
        Arcade => 0,
        Consoles => 1,
        Computers => 2,
        Handhelds => 3,
        Favourites => 4,
        Settings => 5,
    };
    Ok((style.id, style.colour, tag))
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
    let taxonomy: serde_json::Value =
        serde_json::from_str(include_str!("../../catalog/data/system_taxonomy.json"))?;
    let mut sections = BTreeMap::new();
    for system in taxonomy["systems"].as_array().ok_or("missing systems")? {
        let section = system["section"]
            .as_str()
            .ok_or("missing section")?
            .to_owned();
        sections.insert(
            system["id"].as_str().ok_or("missing system id")?.to_owned(),
            section.clone(),
        );
        if let Some(aliases) = system["aliases"].as_array() {
            for alias in aliases {
                sections.insert(
                    alias.as_str().ok_or("invalid alias")?.to_owned(),
                    section.clone(),
                );
            }
        }
    }
    let mut outputs = BTreeMap::new();
    for (key, card) in index["cards"].as_object_mut().ok_or("missing cards")? {
        let file = card["file"].as_str().ok_or("missing source file")?;
        if Path::new(file).file_name().and_then(|p| p.to_str()) != Some(file) {
            return Err("invalid source filename".into());
        }
        let (id, colour, tag) = style(key, &sections)?;
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
