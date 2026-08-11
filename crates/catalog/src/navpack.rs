// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Immutable little-endian system navigation packs.

#[cfg(feature = "builder")]
use crate::system_shard::{SystemGame, SystemNavigationIndexes};
#[cfg(feature = "builder")]
use std::collections::HashMap;

pub const NAVPACK_SCHEMA_VERSION: u32 = 1;
pub const NAVPACK_HEADER_BYTES: usize = 160;
pub const NAVPACK_ROW_BYTES: usize = 48;
pub const NAVPACK_COLD_BYTES: usize = 32;
pub const NAVPACK_LAUNCH_BYTES: usize = 64;
const MAGIC: &[u8; 8] = b"MGKNAVP1";
const ENDIAN_MARKER: u32 = 0x0102_0304;
const NONE_U32: u32 = u32::MAX;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavPackIdentity {
    pub system_id: String,
    pub generation: u64,
    pub games: usize,
    pub launches: usize,
}

#[cfg(feature = "builder")]
#[derive(Default)]
struct Strings {
    bytes: Vec<u8>,
    refs: HashMap<String, (u32, u32)>,
}

#[cfg(feature = "builder")]
impl Strings {
    fn intern(&mut self, value: &str) -> Result<(u32, u32), String> {
        if let Some(reference) = self.refs.get(value) {
            return Ok(*reference);
        }
        let offset = u32::try_from(self.bytes.len())
            .map_err(|_| "NavPack string table exceeds 32-bit offsets")?;
        let length =
            u32::try_from(value.len()).map_err(|_| "NavPack string exceeds 32-bit length")?;
        self.bytes.extend_from_slice(value.as_bytes());
        self.refs.insert(value.to_string(), (offset, length));
        Ok((offset, length))
    }
}

#[cfg(feature = "builder")]
pub fn encode(
    system_id: &str,
    generation: u64,
    games: &[SystemGame],
    indexes: &SystemNavigationIndexes,
) -> Result<Vec<u8>, String> {
    let game_count = u32::try_from(games.len()).map_err(|_| "too many NavPack rows")?;
    let mut strings = Strings::default();
    let system_ref = strings.intern(system_id)?;
    let mut rows = Vec::with_capacity(games.len().saturating_mul(NAVPACK_ROW_BYTES));
    let mut cold = Vec::with_capacity(games.len().saturating_mul(NAVPACK_COLD_BYTES));
    let launch_count = games
        .iter()
        .filter(|game| game.launch_plan.is_some())
        .count();
    let mut launches = Vec::with_capacity(launch_count.saturating_mul(NAVPACK_LAUNCH_BYTES));
    let mut next_launch = 0u32;
    for (ordinal, game) in games.iter().enumerate() {
        for value in [
            game.title.as_str(),
            game.launch_ref.as_str(),
            game.preview_archive_path.as_str(),
            game.preview_asset_key.as_str(),
        ] {
            put_ref(&mut rows, strings.intern(value)?);
        }
        put_u32(
            &mut rows,
            u32::try_from(ordinal).map_err(|_| "NavPack ordinal exceeds 32 bits")?,
        );
        let launch_index = if game.launch_plan.is_some() {
            let index = next_launch;
            next_launch = next_launch
                .checked_add(1)
                .ok_or("NavPack launch count overflow")?;
            index
        } else {
            NONE_U32
        };
        put_u32(&mut rows, launch_index);
        put_u32(
            &mut rows,
            u32::from(game.has_preview) | (u32::from(game.is_new) << 1),
        );
        put_u32(&mut rows, 0);

        put_u16(&mut cold, game.year.unwrap_or(u16::MAX));
        cold.push(game.players.unwrap_or(u8::MAX));
        cold.push(0);
        put_ref(&mut cold, strings.intern(&game.manufacturer)?);
        put_ref(&mut cold, strings.intern(&game.category)?);
        put_ref(&mut cold, strings.intern(&game.control)?);
        put_u32(&mut cold, 0);

        if let Some(plan) = &game.launch_plan {
            for value in [
                plan.launch_ref.as_str(),
                plan.title.as_str(),
                plan.system_id.as_str(),
                plan.core_path.as_str(),
                plan.payload_path.as_str(),
                plan.mount_kind.as_str(),
            ] {
                put_ref(&mut launches, strings.intern(value)?);
            }
            launches.push(plan.mount_index);
            launches.push(plan.delay_secs);
            launches.extend_from_slice(&[0; NAVPACK_LAUNCH_BYTES - 50]);
        }
    }
    let mut index_bytes = Vec::new();
    encode_ordinals(&mut index_bytes, &indexes.title_ordinals)?;
    encode_ordinals(&mut index_bytes, &indexes.preview_ordinals)?;
    encode_ordinals(&mut index_bytes, &indexes.launch_ordinals)?;
    encode_string_postings(&mut index_bytes, &indexes.categories, &mut strings)?;
    encode_u16_postings(&mut index_bytes, &indexes.decades)?;
    encode_string_postings(&mut index_bytes, &indexes.manufacturers, &mut strings)?;
    encode_u8_postings(&mut index_bytes, &indexes.players)?;
    encode_string_postings(&mut index_bytes, &indexes.controls, &mut strings)?;

    let rows_offset = NAVPACK_HEADER_BYTES;
    let cold_offset = aligned_after(rows_offset, rows.len())?;
    let launch_offset = aligned_after(cold_offset, cold.len())?;
    let index_offset = aligned_after(launch_offset, launches.len())?;
    let strings_offset = aligned_after(index_offset, index_bytes.len())?;
    let total = strings_offset
        .checked_add(strings.bytes.len())
        .ok_or("NavPack size overflow")?;
    let mut output = vec![0; total];
    output[rows_offset..rows_offset + rows.len()].copy_from_slice(&rows);
    output[cold_offset..cold_offset + cold.len()].copy_from_slice(&cold);
    output[launch_offset..launch_offset + launches.len()].copy_from_slice(&launches);
    output[index_offset..index_offset + index_bytes.len()].copy_from_slice(&index_bytes);
    output[strings_offset..].copy_from_slice(&strings.bytes);
    let mut header = Vec::with_capacity(NAVPACK_HEADER_BYTES);
    header.extend_from_slice(MAGIC);
    put_u32(&mut header, NAVPACK_SCHEMA_VERSION);
    put_u32(&mut header, NAVPACK_HEADER_BYTES as u32);
    put_u32(&mut header, ENDIAN_MARKER);
    put_u32(&mut header, NAVPACK_ROW_BYTES as u32);
    put_u32(&mut header, NAVPACK_COLD_BYTES as u32);
    put_u32(&mut header, NAVPACK_LAUNCH_BYTES as u32);
    put_u64(&mut header, generation);
    put_u32(&mut header, game_count);
    put_u32(
        &mut header,
        u32::try_from(launch_count).map_err(|_| "too many NavPack launch plans")?,
    );
    put_ref(&mut header, system_ref);
    for (offset, bytes) in [
        (rows_offset, rows.len()),
        (cold_offset, cold.len()),
        (launch_offset, launches.len()),
        (index_offset, index_bytes.len()),
        (strings_offset, strings.bytes.len()),
    ] {
        put_u64(
            &mut header,
            u64::try_from(offset).map_err(|_| "NavPack offset overflow")?,
        );
        put_u64(
            &mut header,
            u64::try_from(bytes).map_err(|_| "NavPack length overflow")?,
        );
    }
    header.resize(NAVPACK_HEADER_BYTES, 0);
    output[..NAVPACK_HEADER_BYTES].copy_from_slice(&header);
    validate(&output, system_id, generation, games.len())?;
    Ok(output)
}

pub fn validate(
    bytes: &[u8],
    expected_system_id: &str,
    expected_generation: u64,
    expected_games: usize,
) -> Result<NavPackIdentity, String> {
    if bytes.len() < NAVPACK_HEADER_BYTES || &bytes[..8] != MAGIC {
        return Err("invalid NavPack header".into());
    }
    let version = read_u32(bytes, 8)?;
    let header_bytes = read_u32(bytes, 12)? as usize;
    if version != NAVPACK_SCHEMA_VERSION || header_bytes != NAVPACK_HEADER_BYTES {
        return Err("unsupported NavPack schema or header size".into());
    }
    if read_u32(bytes, 16)? != ENDIAN_MARKER
        || read_u32(bytes, 20)? as usize != NAVPACK_ROW_BYTES
        || read_u32(bytes, 24)? as usize != NAVPACK_COLD_BYTES
        || read_u32(bytes, 28)? as usize != NAVPACK_LAUNCH_BYTES
    {
        return Err("invalid NavPack layout constants".into());
    }
    let generation = read_u64(bytes, 32)?;
    let games = read_u32(bytes, 40)? as usize;
    let launches = read_u32(bytes, 44)? as usize;
    if generation != expected_generation || games != expected_games {
        return Err("NavPack generation or row count mismatch".into());
    }
    let system_ref = read_ref(bytes, 48)?;
    let sections = (0..5)
        .map(|index| read_section(bytes, 56 + index * 16))
        .collect::<Result<Vec<_>, _>>()?;
    for window in sections.windows(2) {
        if window[0].0 + window[0].1 > window[1].0 {
            return Err("overlapping or unordered NavPack sections".into());
        }
    }
    let (rows_offset, rows_bytes) = sections[0];
    let (cold_offset, cold_bytes) = sections[1];
    let (launch_offset, launch_bytes) = sections[2];
    let (index_offset, index_bytes) = sections[3];
    let (strings_offset, strings_bytes) = sections[4];
    if rows_bytes != games.saturating_mul(NAVPACK_ROW_BYTES)
        || cold_bytes != games.saturating_mul(NAVPACK_COLD_BYTES)
        || launch_bytes != launches.saturating_mul(NAVPACK_LAUNCH_BYTES)
        || strings_offset + strings_bytes > bytes.len()
    {
        return Err("invalid NavPack section size".into());
    }
    let strings = &bytes[strings_offset..strings_offset + strings_bytes];
    if read_string(strings, system_ref)? != expected_system_id {
        return Err("NavPack system identity mismatch".into());
    }
    for ordinal in 0..games {
        let row = rows_offset + ordinal * NAVPACK_ROW_BYTES;
        for offset in [0, 8, 16, 24] {
            read_string(strings, read_ref(bytes, row + offset)?)?;
        }
        if read_u32(bytes, row + 32)? as usize != ordinal {
            return Err("NavPack row metadata ordinal mismatch".into());
        }
        let launch = read_u32(bytes, row + 36)?;
        if launch != NONE_U32 && launch as usize >= launches {
            return Err("NavPack row launch index is out of bounds".into());
        }
        let metadata = cold_offset + ordinal * NAVPACK_COLD_BYTES;
        for offset in [4, 12, 20] {
            read_string(strings, read_ref(bytes, metadata + offset)?)?;
        }
    }
    for launch in 0..launches {
        let record = launch_offset + launch * NAVPACK_LAUNCH_BYTES;
        for offset in [0, 8, 16, 24, 32, 40] {
            read_string(strings, read_ref(bytes, record + offset)?)?;
        }
    }
    validate_indexes(
        &bytes[index_offset..index_offset + index_bytes],
        strings,
        games,
    )?;
    Ok(NavPackIdentity {
        system_id: expected_system_id.to_string(),
        generation,
        games,
        launches,
    })
}

#[cfg(feature = "builder")]
fn encode_ordinals(output: &mut Vec<u8>, ordinals: &[u32]) -> Result<(), String> {
    put_u32(
        output,
        u32::try_from(ordinals.len()).map_err(|_| "too many NavPack ordinals")?,
    );
    for ordinal in ordinals {
        put_u32(output, *ordinal);
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn encode_string_postings(
    output: &mut Vec<u8>,
    groups: &[(String, Vec<u32>)],
    strings: &mut Strings,
) -> Result<(), String> {
    put_u32(
        output,
        u32::try_from(groups.len()).map_err(|_| "too many NavPack postings")?,
    );
    for (value, ordinals) in groups {
        put_ref(output, strings.intern(value)?);
        encode_ordinals(output, ordinals)?;
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn encode_u16_postings(output: &mut Vec<u8>, groups: &[(u16, Vec<u32>)]) -> Result<(), String> {
    put_u32(
        output,
        u32::try_from(groups.len()).map_err(|_| "too many NavPack postings")?,
    );
    for (value, ordinals) in groups {
        put_u16(output, *value);
        put_u16(output, 0);
        encode_ordinals(output, ordinals)?;
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn encode_u8_postings(output: &mut Vec<u8>, groups: &[(u8, Vec<u32>)]) -> Result<(), String> {
    put_u32(
        output,
        u32::try_from(groups.len()).map_err(|_| "too many NavPack postings")?,
    );
    for (value, ordinals) in groups {
        output.push(*value);
        output.extend_from_slice(&[0; 3]);
        encode_ordinals(output, ordinals)?;
    }
    Ok(())
}

fn validate_indexes(mut bytes: &[u8], strings: &[u8], games: usize) -> Result<(), String> {
    for _ in 0..3 {
        take_ordinals(&mut bytes, games)?;
    }
    take_string_postings(&mut bytes, strings, games)?;
    take_numeric_postings(&mut bytes, 4, games)?;
    take_string_postings(&mut bytes, strings, games)?;
    take_numeric_postings(&mut bytes, 4, games)?;
    take_string_postings(&mut bytes, strings, games)?;
    if !bytes.is_empty() {
        return Err("NavPack index section has trailing bytes".into());
    }
    Ok(())
}

fn take_ordinals(bytes: &mut &[u8], games: usize) -> Result<(), String> {
    let count = take_u32(bytes)? as usize;
    for _ in 0..count {
        if take_u32(bytes)? as usize >= games {
            return Err("NavPack persisted index ordinal is out of bounds".into());
        }
    }
    Ok(())
}

fn take_string_postings(bytes: &mut &[u8], strings: &[u8], games: usize) -> Result<(), String> {
    let count = take_u32(bytes)? as usize;
    for _ in 0..count {
        let reference = (take_u32(bytes)?, take_u32(bytes)?);
        read_string(strings, reference)?;
        take_ordinals(bytes, games)?;
    }
    Ok(())
}

fn take_numeric_postings(bytes: &mut &[u8], key_bytes: usize, games: usize) -> Result<(), String> {
    let count = take_u32(bytes)? as usize;
    for _ in 0..count {
        take(bytes, key_bytes)?;
        take_ordinals(bytes, games)?;
    }
    Ok(())
}

#[cfg(feature = "builder")]
fn aligned_after(offset: usize, bytes: usize) -> Result<usize, String> {
    offset
        .checked_add(bytes)
        .and_then(|value| value.checked_add(7))
        .map(|value| value & !7)
        .ok_or_else(|| "NavPack offset overflow".into())
}

#[cfg(feature = "builder")]
fn put_ref(output: &mut Vec<u8>, reference: (u32, u32)) {
    put_u32(output, reference.0);
    put_u32(output, reference.1);
}

#[cfg(feature = "builder")]
fn put_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "builder")]
fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

#[cfg(feature = "builder")]
fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("truncated NavPack u32")?
            .try_into()
            .map_err(|_| "invalid NavPack u32")?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or("truncated NavPack u64")?
            .try_into()
            .map_err(|_| "invalid NavPack u64")?,
    ))
}

fn read_ref(bytes: &[u8], offset: usize) -> Result<(u32, u32), String> {
    Ok((read_u32(bytes, offset)?, read_u32(bytes, offset + 4)?))
}

fn read_section(bytes: &[u8], offset: usize) -> Result<(usize, usize), String> {
    let start =
        usize::try_from(read_u64(bytes, offset)?).map_err(|_| "NavPack offset too large")?;
    let length =
        usize::try_from(read_u64(bytes, offset + 8)?).map_err(|_| "NavPack section too large")?;
    start
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| "NavPack section is out of bounds".to_string())?;
    Ok((start, length))
}

fn read_string(strings: &[u8], reference: (u32, u32)) -> Result<&str, String> {
    let start = reference.0 as usize;
    let end = start
        .checked_add(reference.1 as usize)
        .filter(|end| *end <= strings.len())
        .ok_or("NavPack string reference is out of bounds")?;
    std::str::from_utf8(&strings[start..end]).map_err(|_| "NavPack string is not UTF-8".into())
}

fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Result<&'a [u8], String> {
    let (head, tail) = bytes
        .split_at_checked(count)
        .ok_or("truncated NavPack index")?;
    *bytes = tail;
    Ok(head)
}

fn take_u32(bytes: &mut &[u8]) -> Result<u32, String> {
    let raw = take(bytes, 4)?;
    Ok(u32::from_le_bytes(
        raw.try_into().map_err(|_| "invalid NavPack index u32")?,
    ))
}

#[cfg(all(test, feature = "builder"))]
mod tests {
    use super::*;
    use crate::system_shard::{SystemLaunchPlan, build_navigation_indexes};

    fn fixture() -> Vec<SystemGame> {
        vec![
            SystemGame {
                stable_key: "c64:a".into(),
                title: "Alpha".into(),
                launch_ref: "magik-plan:c64:a".into(),
                preview_archive_path: "/preview/c64.zip".into(),
                preview_asset_key: "Alpha".into(),
                has_preview: true,
                year: Some(1984),
                manufacturer: "Example".into(),
                category: "Action".into(),
                players: Some(1),
                control: "Joystick".into(),
                is_new: false,
                launch_plan: Some(SystemLaunchPlan {
                    launch_ref: "magik-plan:c64:a".into(),
                    title: "Alpha".into(),
                    system_id: "c64".into(),
                    core_path: "C64".into(),
                    payload_path: "/games/a.d64".into(),
                    mount_kind: "mount-image".into(),
                    mount_index: 0,
                    delay_secs: 1,
                }),
            },
            SystemGame {
                stable_key: "c64:b".into(),
                title: "Beta".into(),
                launch_ref: "/games/b.d64".into(),
                preview_archive_path: String::new(),
                preview_asset_key: String::new(),
                has_preview: false,
                year: None,
                manufacturer: String::new(),
                category: String::new(),
                players: None,
                control: String::new(),
                is_new: true,
                launch_plan: None,
            },
        ]
    }

    #[test]
    fn navpack_roundtrip_validates_identity_tables_and_indexes() {
        let games = fixture();
        let indexes = build_navigation_indexes(&games).unwrap();
        let encoded = encode("c64", 9, &games, &indexes).unwrap();
        let identity = validate(&encoded, "c64", 9, 2).unwrap();
        assert_eq!(identity.games, 2);
        assert_eq!(identity.launches, 1);
        assert_eq!(encoded.len() % 1, 0);
    }

    #[test]
    fn navpack_rejects_corrupt_offsets_and_generation() {
        let games = fixture();
        let indexes = build_navigation_indexes(&games).unwrap();
        let mut encoded = encode("c64", 9, &games, &indexes).unwrap();
        assert!(validate(&encoded, "c64", 10, 2).is_err());
        encoded[56..64].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(validate(&encoded, "c64", 9, 2).is_err());
    }
}
