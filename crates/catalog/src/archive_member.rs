// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Explicit launch references for payloads stored inside ZIP archives.

use serde::{Deserialize, Serialize};

pub const ARCHIVE_MEMBER_PREFIX: &str = "magik-archive-member:";

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ArchiveMemberRef {
    pub archive_path: String,
    pub member_path: String,
    pub local_header_offset: u64,
    pub compression_method: u16,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub crc32: u32,
}

pub fn encode_archive_member_ref(member: &ArchiveMemberRef) -> Result<String, String> {
    serde_json::to_string(member)
        .map(|json| format!("{ARCHIVE_MEMBER_PREFIX}{json}"))
        .map_err(|error| format!("encode archive member launch ref: {error}"))
}

pub fn decode_archive_member_ref(value: &str) -> Result<Option<ArchiveMemberRef>, String> {
    let Some(json) = value.strip_prefix(ARCHIVE_MEMBER_PREFIX) else {
        return Ok(None);
    };
    serde_json::from_str(json)
        .map(Some)
        .map_err(|error| format!("invalid archive member launch ref: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_member_ref_round_trips_paths_without_separator_assumptions() {
        let member = ArchiveMemberRef {
            archive_path: "/games/a::b.zip".to_string(),
            member_path: "nested/a:b.bin".to_string(),
            local_header_offset: 42,
            compression_method: 8,
            compressed_size: 20,
            uncompressed_size: 100,
            crc32: 0x1234_5678,
        };
        let encoded = encode_archive_member_ref(&member).expect("encode");
        assert_eq!(decode_archive_member_ref(&encoded), Ok(Some(member)));
    }
}
