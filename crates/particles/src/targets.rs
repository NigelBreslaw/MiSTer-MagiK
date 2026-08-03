// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Versioned sidecars for deterministic grouped point-cloud targets.

const PARTICLE_GROUP_MAGIC: &[u8; 8] = b"PGROUP1\0";
const PARTICLE_GROUP_HEADER_BYTES: usize = 16;
const PARTICLE_GROUP_VERSION: u16 = 1;
const PARTICLE_GROUP_RECORD_BYTES: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParticleGroupSpan {
    pub id: u8,
    pub start: usize,
    pub count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticleGroups {
    ids: Vec<u8>,
    spans: Vec<ParticleGroupSpan>,
}

impl ParticleGroups {
    #[must_use]
    pub fn ids(&self) -> &[u8] {
        &self.ids
    }

    #[must_use]
    pub fn spans(&self) -> &[ParticleGroupSpan] {
        &self.spans
    }
}

/// Decode a strict `PGROUP1` sidecar aligned one-to-one with a point cloud.
///
/// Records must be ordered as contiguous, four-particle-aligned group spans so
/// each group can be projected with the packed SIMD path without repacking.
pub fn decode_particle_groups(
    bytes: &[u8],
    expected_count: usize,
    group_count: u8,
) -> Result<ParticleGroups, String> {
    if bytes.len() < PARTICLE_GROUP_HEADER_BYTES || &bytes[..8] != PARTICLE_GROUP_MAGIC {
        return Err("particle group header is invalid".into());
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    let stride = u16::from_le_bytes([bytes[10], bytes[11]]);
    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if version != PARTICLE_GROUP_VERSION || stride != PARTICLE_GROUP_RECORD_BYTES {
        return Err(format!(
            "particle group contract mismatch: version={version} stride={stride} count={count}"
        ));
    }
    if group_count == 0 || count != expected_count {
        return Err(format!(
            "particle group count {count} does not match expected {expected_count}"
        ));
    }
    let expected_len = PARTICLE_GROUP_HEADER_BYTES.saturating_add(count);
    if bytes.len() != expected_len {
        return Err(format!(
            "particle group length {} does not match expected {expected_len}",
            bytes.len()
        ));
    }
    let ids = bytes[PARTICLE_GROUP_HEADER_BYTES..].to_vec();
    if ids.iter().any(|id| *id >= group_count) {
        return Err("particle group contains an out-of-range id".into());
    }
    let mut spans = Vec::with_capacity(usize::from(group_count));
    let mut start = 0;
    while start < ids.len() {
        let id = ids[start];
        let mut end = start + 1;
        while end < ids.len() && ids[end] == id {
            end += 1;
        }
        if start % 4 != 0 || (end - start) % 4 != 0 {
            return Err(format!("particle group {id} is not four-lane aligned"));
        }
        if spans.iter().any(|span: &ParticleGroupSpan| span.id == id) {
            return Err(format!("particle group {id} is not contiguous"));
        }
        spans.push(ParticleGroupSpan {
            id,
            start,
            count: end - start,
        });
        start = end;
    }
    if spans.len() != usize::from(group_count)
        || spans.iter().enumerate().any(|(index, span)| usize::from(span.id) != index)
    {
        return Err("particle groups must contain each id once in ascending order".into());
    }
    Ok(ParticleGroups { ids, spans })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(ids: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::from(&PARTICLE_GROUP_MAGIC[..]);
        bytes.extend_from_slice(&PARTICLE_GROUP_VERSION.to_le_bytes());
        bytes.extend_from_slice(&PARTICLE_GROUP_RECORD_BYTES.to_le_bytes());
        bytes.extend_from_slice(&(ids.len() as u32).to_le_bytes());
        bytes.extend_from_slice(ids);
        bytes
    }

    #[test]
    fn grouped_target_requires_ordered_four_lane_spans() {
        let ids = [0; 4].into_iter().chain([1; 8]).collect::<Vec<_>>();
        let groups = decode_particle_groups(&encoded(&ids), 12, 2).unwrap();
        assert_eq!(
            groups.spans()[0],
            ParticleGroupSpan {
                id: 0,
                start: 0,
                count: 4,
            }
        );
        assert_eq!(
            groups.spans()[1],
            ParticleGroupSpan {
                id: 1,
                start: 4,
                count: 8,
            }
        );

        assert!(decode_particle_groups(&encoded(&[0, 0, 1, 1]), 4, 2).is_err());
        assert!(
            decode_particle_groups(
                &encoded(&[0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0]),
                12,
                2
            )
            .is_err()
        );
        assert!(decode_particle_groups(&encoded(&[0; 4]), 4, 2).is_err());
    }

    #[test]
    fn grouped_target_rejects_contract_and_count_mismatches() {
        let mut bytes = encoded(&[0; 4]);
        bytes[8] = 2;
        assert!(decode_particle_groups(&bytes, 4, 1).is_err());
        assert!(decode_particle_groups(&encoded(&[0; 4]), 8, 1).is_err());
        assert!(decode_particle_groups(&encoded(&[2; 4]), 4, 1).is_err());
    }

    #[test]
    fn checked_in_intro_groups_match_the_six_track_contract() {
        for bytes in [
            include_bytes!("../assets/intro/mister.pgroup").as_slice(),
            include_bytes!("../assets/intro/magik.pgroup").as_slice(),
        ] {
            let groups = decode_particle_groups(bytes, 40_960, 6).unwrap();
            assert_eq!(groups.spans().len(), 6);
            assert_eq!(
                groups
                    .spans()
                    .iter()
                    .map(|span| span.count)
                    .collect::<Vec<_>>(),
                [8_192, 4_096, 8_192, 4_096, 8_192, 8_192]
            );
        }
    }
}
