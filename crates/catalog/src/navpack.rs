// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Immutable little-endian system navigation packs.

#[cfg(feature = "builder")]
use crate::system_shard::{SystemGame, SystemNavigationIndexes};
#[cfg(feature = "builder")]
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

pub const NAVPACK_SCHEMA_VERSION: u32 = 2;
pub const NAVPACK_HEADER_BYTES: usize = 160;
pub const NAVPACK_ROW_BYTES: usize = 48;
pub const NAVPACK_COLD_BYTES: usize = 32;
pub const NAVPACK_LAUNCH_BYTES: usize = 64;
pub const NAVPACK_ENTRY_VIEWPORT_ROWS: usize = 10;
const MAGIC: &[u8; 8] = b"MGKNAVP2";
const PRELUDE_MAGIC: &[u8; 8] = b"MGKPREL1";
const PRELUDE_VERSION: u32 = 1;
const PRELUDE_HEADER_BYTES: usize = 48;
const PRELUDE_FLAG_TERMINAL_EMPTY: u32 = 1;
const PRELUDE_FLAG_EXACT_PREVIEW: u32 = 2;
const ENDIAN_MARKER: u32 = 0x0102_0304;
const NONE_U32: u32 = u32::MAX;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavPackIdentity {
    pub system_id: String,
    pub generation: u64,
    pub games: usize,
    pub launches: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NavPackPreviewIdentityRef<'a> {
    pub ordinal: usize,
    pub title: &'a str,
    pub preview_archive_path: &'a str,
    pub preview_asset_key: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavPackEntryPreludeRef<'a> {
    pub first_viewport_ordinals: Vec<usize>,
    pub selected_preview: Option<NavPackPreviewIdentityRef<'a>>,
    pub terminal_empty: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct NavPackOpenTiming {
    pub file_open_us: u64,
    pub mmap_us: u64,
    pub header_validation_us: u64,
    pub total_us: u64,
}

#[derive(Clone, Debug)]
pub struct MappedNavPack {
    mapping: Arc<memmap2::Mmap>,
    identity: NavPackIdentity,
    rows_offset: usize,
    cold_offset: usize,
    launch_offset: usize,
    strings_offset: usize,
    strings_bytes: usize,
    prelude_offset: usize,
    prelude_bytes: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct NavPackRowRef<'a> {
    pub title: &'a str,
    pub launch_ref: &'a str,
    pub preview_archive_path: &'a str,
    pub preview_asset_key: &'a str,
    pub has_preview: bool,
    pub is_new: bool,
    pub launch_index: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct NavPackMetadataRef<'a> {
    pub year: Option<u16>,
    pub manufacturer: &'a str,
    pub category: &'a str,
    pub players: Option<u8>,
    pub control: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct NavPackLaunchRef<'a> {
    pub launch_ref: &'a str,
    pub title: &'a str,
    pub system_id: &'a str,
    pub core_path: &'a str,
    pub payload_path: &'a str,
    pub mount_kind: &'a str,
    pub mount_index: u8,
    pub delay_secs: u8,
}

impl MappedNavPack {
    pub fn open(
        path: &Path,
        expected_bytes: u64,
        expected_system_id: &str,
        expected_generation: u64,
        expected_games: usize,
    ) -> Result<(Self, NavPackOpenTiming), String> {
        let total_started = std::time::Instant::now();
        let file_started = std::time::Instant::now();
        let file = File::open(path).map_err(|error| format!("open NavPack: {error}"))?;
        let actual_bytes = file
            .metadata()
            .map_err(|error| format!("stat NavPack: {error}"))?
            .len();
        if actual_bytes != expected_bytes {
            return Err(format!(
                "NavPack size mismatch: expected {expected_bytes}, found {actual_bytes}"
            ));
        }
        let file_open_us = elapsed_us(file_started);
        let mmap_started = std::time::Instant::now();
        // SAFETY: published NavPacks are immutable generation-scoped files. The mapping owns an
        // open-file-derived VM object, and garbage collection retains every manifest generation
        // while an Arc<MappedNavPack> can be reachable by the launcher.
        let mapping = unsafe { memmap2::MmapOptions::new().map(&file) }
            .map_err(|error| format!("mmap NavPack: {error}"))?;
        let mmap_us = elapsed_us(mmap_started);
        let header_started = std::time::Instant::now();
        let header = validate_header(
            &mapping,
            expected_system_id,
            expected_generation,
            expected_games,
        )?;
        let header_validation_us = elapsed_us(header_started);
        Ok((
            Self {
                mapping: Arc::new(mapping),
                identity: header.identity,
                rows_offset: header.rows_offset,
                cold_offset: header.cold_offset,
                launch_offset: header.launch_offset,
                strings_offset: header.strings_offset,
                strings_bytes: header.strings_bytes,
                prelude_offset: header.prelude_offset,
                prelude_bytes: header.prelude_bytes,
            },
            NavPackOpenTiming {
                file_open_us,
                mmap_us,
                header_validation_us,
                total_us: elapsed_us(total_started),
            },
        ))
    }

    pub fn mapping(&self) -> &Arc<memmap2::Mmap> {
        &self.mapping
    }

    pub fn identity(&self) -> &NavPackIdentity {
        &self.identity
    }

    pub fn entry_prelude(&self) -> Result<NavPackEntryPreludeRef<'_>, String> {
        parse_entry_prelude(
            self.mapping
                .get(self.prelude_offset..self.prelude_offset + self.prelude_bytes)
                .ok_or("NavPack entry prelude is out of bounds")?,
            self.identity.games,
        )
    }

    /// Validates the bounded prelude and faults every hot row/string page it names.
    pub fn fault_entry_viewport(&self) -> Result<NavPackEntryPreludeRef<'_>, String> {
        let prelude = self.entry_prelude()?;
        for ordinal in &prelude.first_viewport_ordinals {
            let _ = self.row(*ordinal)?;
        }
        Ok(prelude)
    }

    pub fn row(&self, ordinal: usize) -> Result<NavPackRowRef<'_>, String> {
        if ordinal >= self.identity.games {
            return Err("NavPack row ordinal is out of bounds".into());
        }
        let offset = self.rows_offset + ordinal * NAVPACK_ROW_BYTES;
        let strings = self.strings();
        let flags = read_u32(&self.mapping, offset + 40)?;
        let launch = read_u32(&self.mapping, offset + 36)?;
        Ok(NavPackRowRef {
            title: read_string(strings, read_ref(&self.mapping, offset)?)?,
            launch_ref: read_string(strings, read_ref(&self.mapping, offset + 8)?)?,
            preview_archive_path: read_string(strings, read_ref(&self.mapping, offset + 16)?)?,
            preview_asset_key: read_string(strings, read_ref(&self.mapping, offset + 24)?)?,
            has_preview: flags & 1 != 0,
            is_new: flags & 2 != 0,
            launch_index: (launch != NONE_U32).then_some(launch as usize),
        })
    }

    pub fn metadata(&self, ordinal: usize) -> Result<NavPackMetadataRef<'_>, String> {
        if ordinal >= self.identity.games {
            return Err("NavPack metadata ordinal is out of bounds".into());
        }
        let offset = self.cold_offset + ordinal * NAVPACK_COLD_BYTES;
        let strings = self.strings();
        let year = read_u16(&self.mapping, offset)?;
        let players = *self
            .mapping
            .get(offset + 2)
            .ok_or("truncated NavPack players")?;
        Ok(NavPackMetadataRef {
            year: (year != u16::MAX).then_some(year),
            manufacturer: read_string(strings, read_ref(&self.mapping, offset + 4)?)?,
            category: read_string(strings, read_ref(&self.mapping, offset + 12)?)?,
            players: (players != u8::MAX).then_some(players),
            control: read_string(strings, read_ref(&self.mapping, offset + 20)?)?,
        })
    }

    pub fn launch(&self, index: usize) -> Result<NavPackLaunchRef<'_>, String> {
        if index >= self.identity.launches {
            return Err("NavPack launch index is out of bounds".into());
        }
        let offset = self.launch_offset + index * NAVPACK_LAUNCH_BYTES;
        let strings = self.strings();
        Ok(NavPackLaunchRef {
            launch_ref: read_string(strings, read_ref(&self.mapping, offset)?)?,
            title: read_string(strings, read_ref(&self.mapping, offset + 8)?)?,
            system_id: read_string(strings, read_ref(&self.mapping, offset + 16)?)?,
            core_path: read_string(strings, read_ref(&self.mapping, offset + 24)?)?,
            payload_path: read_string(strings, read_ref(&self.mapping, offset + 32)?)?,
            mount_kind: read_string(strings, read_ref(&self.mapping, offset + 40)?)?,
            mount_index: *self
                .mapping
                .get(offset + 48)
                .ok_or("truncated NavPack mount index")?,
            delay_secs: *self
                .mapping
                .get(offset + 49)
                .ok_or("truncated NavPack delay")?,
        })
    }

    fn strings(&self) -> &[u8] {
        &self.mapping[self.strings_offset..self.strings_offset + self.strings_bytes]
    }
}

struct CheckedHeader {
    identity: NavPackIdentity,
    rows_offset: usize,
    cold_offset: usize,
    launch_offset: usize,
    strings_offset: usize,
    strings_bytes: usize,
    prelude_offset: usize,
    prelude_bytes: usize,
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

    let prelude = encode_entry_prelude(games, indexes)?;
    let prelude_offset = NAVPACK_HEADER_BYTES;
    let rows_offset = aligned_after(prelude_offset, prelude.len())?;
    let cold_offset = aligned_after(rows_offset, rows.len())?;
    let launch_offset = aligned_after(cold_offset, cold.len())?;
    let index_offset = aligned_after(launch_offset, launches.len())?;
    let strings_offset = aligned_after(index_offset, index_bytes.len())?;
    let total = strings_offset
        .checked_add(strings.bytes.len())
        .ok_or("NavPack size overflow")?;
    let mut output = vec![0; total];
    output[prelude_offset..prelude_offset + prelude.len()].copy_from_slice(&prelude);
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
    put_u64(
        &mut header,
        u64::try_from(prelude_offset).map_err(|_| "NavPack prelude offset overflow")?,
    );
    put_u64(
        &mut header,
        u64::try_from(prelude.len()).map_err(|_| "NavPack prelude length overflow")?,
    );
    header.resize(NAVPACK_HEADER_BYTES, 0);
    output[..NAVPACK_HEADER_BYTES].copy_from_slice(&header);
    validate(&output, system_id, generation, games.len())?;
    Ok(output)
}

#[cfg(feature = "builder")]
fn encode_entry_prelude(
    games: &[SystemGame],
    indexes: &SystemNavigationIndexes,
) -> Result<Vec<u8>, String> {
    let first_viewport = indexes
        .title_ordinals
        .iter()
        .copied()
        .take(NAVPACK_ENTRY_VIEWPORT_ROWS)
        .collect::<Vec<_>>();
    let selected_preview = games.first().filter(|game| {
        game.has_preview
            && !game.preview_archive_path.is_empty()
            && !game.preview_asset_key.is_empty()
    });
    let (selected_ordinal, flags, title, archive, asset_key) = match selected_preview {
        Some(game) => (
            0u32,
            PRELUDE_FLAG_EXACT_PREVIEW,
            game.title.as_bytes(),
            game.preview_archive_path.as_bytes(),
            game.preview_asset_key.as_bytes(),
        ),
        None => (
            NONE_U32,
            PRELUDE_FLAG_TERMINAL_EMPTY,
            &[][..],
            &[][..],
            &[][..],
        ),
    };
    let total_bytes = PRELUDE_HEADER_BYTES
        .checked_add(first_viewport.len().saturating_mul(4))
        .and_then(|value| value.checked_add(title.len()))
        .and_then(|value| value.checked_add(archive.len()))
        .and_then(|value| value.checked_add(asset_key.len()))
        .ok_or("NavPack entry prelude size overflow")?;
    let mut output = Vec::with_capacity(total_bytes);
    output.extend_from_slice(PRELUDE_MAGIC);
    put_u32(&mut output, PRELUDE_VERSION);
    put_u32(
        &mut output,
        u32::try_from(total_bytes).map_err(|_| "NavPack entry prelude exceeds 32 bits")?,
    );
    put_u32(
        &mut output,
        u32::try_from(first_viewport.len()).map_err(|_| "too many entry viewport ordinals")?,
    );
    put_u32(&mut output, selected_ordinal);
    put_u32(&mut output, flags);
    put_u32(&mut output, 0);
    for value in [title.len(), archive.len(), asset_key.len()] {
        put_u32(
            &mut output,
            u32::try_from(value).map_err(|_| "NavPack entry identity exceeds 32 bits")?,
        );
    }
    put_u32(&mut output, 0);
    for ordinal in first_viewport {
        put_u32(&mut output, ordinal);
    }
    output.extend_from_slice(title);
    output.extend_from_slice(archive);
    output.extend_from_slice(asset_key);
    debug_assert_eq!(output.len(), total_bytes);
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
    let (prelude_offset, prelude_bytes) = read_section(bytes, 136)?;
    if prelude_offset != NAVPACK_HEADER_BYTES || prelude_offset + prelude_bytes > sections[0].0 {
        return Err("NavPack entry prelude is not the leading section".into());
    }
    let prelude = parse_entry_prelude(
        &bytes[prelude_offset..prelude_offset + prelude_bytes],
        games,
    )?;
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
    if let Some(selected) = prelude.selected_preview {
        let row = rows_offset + selected.ordinal * NAVPACK_ROW_BYTES;
        let flags = read_u32(bytes, row + 40)?;
        if flags & 1 == 0
            || read_string(strings, read_ref(bytes, row)?)? != selected.title
            || read_string(strings, read_ref(bytes, row + 16)?)? != selected.preview_archive_path
            || read_string(strings, read_ref(bytes, row + 24)?)? != selected.preview_asset_key
        {
            return Err("NavPack entry preview identity does not match its selected row".into());
        }
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

fn validate_header(
    bytes: &[u8],
    expected_system_id: &str,
    expected_generation: u64,
    expected_games: usize,
) -> Result<CheckedHeader, String> {
    if bytes.len() < NAVPACK_HEADER_BYTES || &bytes[..8] != MAGIC {
        return Err("invalid NavPack header".into());
    }
    if read_u32(bytes, 8)? != NAVPACK_SCHEMA_VERSION
        || read_u32(bytes, 12)? as usize != NAVPACK_HEADER_BYTES
    {
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

    let sections = (0..5)
        .map(|index| read_section(bytes, 56 + index * 16))
        .collect::<Result<Vec<_>, _>>()?;
    let (prelude_offset, prelude_bytes) = read_section(bytes, 136)?;
    if prelude_offset != NAVPACK_HEADER_BYTES || prelude_offset + prelude_bytes > sections[0].0 {
        return Err("NavPack entry prelude is not the leading section".into());
    }
    parse_entry_prelude(
        &bytes[prelude_offset..prelude_offset + prelude_bytes],
        games,
    )?;
    for window in sections.windows(2) {
        let end = window[0]
            .0
            .checked_add(window[0].1)
            .ok_or("NavPack section end overflow")?;
        if end > window[1].0 {
            return Err("overlapping or unordered NavPack sections".into());
        }
    }

    let expected_rows = games
        .checked_mul(NAVPACK_ROW_BYTES)
        .ok_or("NavPack row section size overflow")?;
    let expected_cold = games
        .checked_mul(NAVPACK_COLD_BYTES)
        .ok_or("NavPack cold section size overflow")?;
    let expected_launches = launches
        .checked_mul(NAVPACK_LAUNCH_BYTES)
        .ok_or("NavPack launch section size overflow")?;
    if sections[0].1 != expected_rows
        || sections[1].1 != expected_cold
        || sections[2].1 != expected_launches
    {
        return Err("invalid NavPack section size".into());
    }

    let strings_end = sections[4]
        .0
        .checked_add(sections[4].1)
        .ok_or("NavPack string section end overflow")?;
    let strings = bytes
        .get(sections[4].0..strings_end)
        .ok_or("NavPack string section is out of bounds")?;
    if read_string(strings, read_ref(bytes, 48)?)? != expected_system_id {
        return Err("NavPack system identity mismatch".into());
    }

    Ok(CheckedHeader {
        identity: NavPackIdentity {
            system_id: expected_system_id.to_string(),
            generation,
            games,
            launches,
        },
        rows_offset: sections[0].0,
        cold_offset: sections[1].0,
        launch_offset: sections[2].0,
        strings_offset: sections[4].0,
        strings_bytes: sections[4].1,
        prelude_offset,
        prelude_bytes,
    })
}

fn parse_entry_prelude(bytes: &[u8], games: usize) -> Result<NavPackEntryPreludeRef<'_>, String> {
    if bytes.len() < PRELUDE_HEADER_BYTES || &bytes[..8] != PRELUDE_MAGIC {
        return Err("invalid NavPack entry prelude".into());
    }
    if read_u32(bytes, 8)? != PRELUDE_VERSION || read_u32(bytes, 12)? as usize != bytes.len() {
        return Err("unsupported NavPack entry prelude version or size".into());
    }
    let viewport_count = read_u32(bytes, 16)? as usize;
    if viewport_count > NAVPACK_ENTRY_VIEWPORT_ROWS || viewport_count > games {
        return Err("NavPack entry viewport count is out of bounds".into());
    }
    let selected_ordinal = read_u32(bytes, 20)?;
    let flags = read_u32(bytes, 24)?;
    let terminal_empty = flags == PRELUDE_FLAG_TERMINAL_EMPTY;
    let exact_preview = flags == PRELUDE_FLAG_EXACT_PREVIEW;
    if !terminal_empty && !exact_preview {
        return Err("NavPack entry preview state is not terminal".into());
    }
    let title_bytes = read_u32(bytes, 32)? as usize;
    let archive_bytes = read_u32(bytes, 36)? as usize;
    let asset_key_bytes = read_u32(bytes, 40)? as usize;
    let ordinals_end = PRELUDE_HEADER_BYTES
        .checked_add(viewport_count.saturating_mul(4))
        .ok_or("NavPack entry viewport size overflow")?;
    let identity_end = ordinals_end
        .checked_add(title_bytes)
        .and_then(|value| value.checked_add(archive_bytes))
        .and_then(|value| value.checked_add(asset_key_bytes))
        .filter(|value| *value == bytes.len())
        .ok_or("NavPack entry identity size mismatch")?;
    debug_assert_eq!(identity_end, bytes.len());
    let mut first_viewport_ordinals = Vec::with_capacity(viewport_count);
    for index in 0..viewport_count {
        let ordinal = read_u32(bytes, PRELUDE_HEADER_BYTES + index * 4)? as usize;
        if ordinal >= games {
            return Err("NavPack entry viewport ordinal is out of bounds".into());
        }
        first_viewport_ordinals.push(ordinal);
    }
    let mut cursor = ordinals_end;
    let title = std::str::from_utf8(
        bytes
            .get(cursor..cursor + title_bytes)
            .ok_or("truncated NavPack entry title")?,
    )
    .map_err(|_| "NavPack entry title is not UTF-8")?;
    cursor += title_bytes;
    let preview_archive_path = std::str::from_utf8(
        bytes
            .get(cursor..cursor + archive_bytes)
            .ok_or("truncated NavPack entry archive")?,
    )
    .map_err(|_| "NavPack entry archive is not UTF-8")?;
    cursor += archive_bytes;
    let preview_asset_key = std::str::from_utf8(
        bytes
            .get(cursor..cursor + asset_key_bytes)
            .ok_or("truncated NavPack entry asset key")?,
    )
    .map_err(|_| "NavPack entry asset key is not UTF-8")?;
    let selected_preview = if exact_preview {
        let ordinal = usize::try_from(selected_ordinal)
            .ok()
            .filter(|ordinal| *ordinal < games)
            .ok_or("NavPack entry selected ordinal is out of bounds")?;
        if preview_archive_path.is_empty() || preview_asset_key.is_empty() {
            return Err("NavPack exact entry preview identity is empty".into());
        }
        Some(NavPackPreviewIdentityRef {
            ordinal,
            title,
            preview_archive_path,
            preview_asset_key,
        })
    } else {
        if selected_ordinal != NONE_U32
            || title_bytes != 0
            || archive_bytes != 0
            || asset_key_bytes != 0
        {
            return Err("NavPack empty entry preview carries an identity".into());
        }
        None
    };
    Ok(NavPackEntryPreludeRef {
        first_viewport_ordinals,
        selected_preview,
        terminal_empty,
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

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or("truncated NavPack u16")?
            .try_into()
            .map_err(|_| "invalid NavPack u16")?,
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

fn elapsed_us(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[cfg(all(test, feature = "builder"))]
mod tests {
    use super::*;
    use crate::system_shard::{SystemLaunchPlan, build_navigation_indexes};

    fn temp_navpack_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{}.navpack",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

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
    fn navpack_entry_prelude_is_leading_bounded_and_preview_terminal() {
        let games = fixture();
        let indexes = build_navigation_indexes(&games).unwrap();
        let encoded = encode("c64", 9, &games, &indexes).unwrap();
        let path = temp_navpack_path("entry-prelude");
        std::fs::write(&path, &encoded).unwrap();
        let (mapped, _) =
            MappedNavPack::open(&path, encoded.len() as u64, "c64", 9, games.len()).unwrap();
        std::fs::remove_file(path).unwrap();

        let prelude = mapped.fault_entry_viewport().unwrap();
        assert_eq!(prelude.first_viewport_ordinals, vec![0, 1]);
        assert!(!prelude.terminal_empty);
        let selected = prelude.selected_preview.unwrap();
        assert_eq!(selected.ordinal, 0);
        assert_eq!(selected.title, "Alpha");
        assert_eq!(selected.preview_archive_path, "/preview/c64.zip");
        assert_eq!(selected.preview_asset_key, "Alpha");
        assert_eq!(
            read_u64(&encoded, 136).unwrap() as usize,
            NAVPACK_HEADER_BYTES
        );
        assert!((read_u64(&encoded, 144).unwrap() as usize) < 4096);
    }

    #[test]
    fn navpack_entry_prelude_confirms_no_preview_without_an_identity() {
        let mut games = fixture();
        games[0].has_preview = false;
        games[0].preview_archive_path.clear();
        games[0].preview_asset_key.clear();
        let indexes = build_navigation_indexes(&games).unwrap();
        let encoded = encode("c64", 9, &games, &indexes).unwrap();
        let path = temp_navpack_path("empty-entry-prelude");
        std::fs::write(&path, &encoded).unwrap();
        let (mapped, _) =
            MappedNavPack::open(&path, encoded.len() as u64, "c64", 9, games.len()).unwrap();
        std::fs::remove_file(path).unwrap();

        let prelude = mapped.entry_prelude().unwrap();
        assert!(prelude.terminal_empty);
        assert!(prelude.selected_preview.is_none());
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

    #[test]
    fn mapped_navpack_keeps_every_c64_shaped_ordinal_synchronously_addressable() {
        const C64_ROWS: usize = 15_089;
        let games = (0..C64_ROWS)
            .map(|ordinal| SystemGame {
                stable_key: format!("c64:{ordinal}"),
                title: format!("C64 Game {ordinal:05}"),
                launch_ref: format!("/games/{ordinal:05}.d64"),
                preview_archive_path: "/preview/c64.zip".into(),
                preview_asset_key: format!("C64 Game {ordinal:05}"),
                has_preview: ordinal % 3 == 0,
                year: Some(1980 + (ordinal % 20) as u16),
                manufacturer: "Example".into(),
                category: "Game".into(),
                players: Some(1),
                control: "Joystick".into(),
                is_new: false,
                launch_plan: None,
            })
            .collect::<Vec<_>>();
        let indexes = build_navigation_indexes(&games).unwrap();
        let encoded = encode("c64", 21, &games, &indexes).unwrap();
        let path = temp_navpack_path("all-c64-rows");
        std::fs::write(&path, &encoded).unwrap();
        let (mapped, _) =
            MappedNavPack::open(&path, encoded.len() as u64, "c64", 21, C64_ROWS).unwrap();
        std::fs::remove_file(&path).unwrap();

        let mut ordinal = 7_919usize;
        for _ in 0..C64_ROWS {
            let row = mapped.row(ordinal).unwrap();
            assert_eq!(row.title, format!("C64 Game {ordinal:05}"));
            assert_eq!(mapped.metadata(ordinal).unwrap().manufacturer, "Example");
            ordinal = (ordinal + 7_919) % C64_ROWS;
        }
        assert!(Arc::strong_count(mapped.mapping()) >= 1);
    }

    #[test]
    fn mapped_navpack_rejects_corrupt_header_offsets_before_row_access() {
        let games = fixture();
        let indexes = build_navigation_indexes(&games).unwrap();
        let mut encoded = encode("c64", 9, &games, &indexes).unwrap();
        encoded[56..64].copy_from_slice(&u64::MAX.to_le_bytes());
        let path = temp_navpack_path("corrupt-header");
        std::fs::write(&path, &encoded).unwrap();
        let error = MappedNavPack::open(&path, encoded.len() as u64, "c64", 9, 2).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert!(error.contains("out of bounds") || error.contains("too large"));
    }
}
