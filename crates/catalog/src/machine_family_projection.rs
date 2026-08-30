// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic contiguous family projection for fast-system rows.

use crate::fast_five_catalog::{FastFiveGameVariant, FastFiveVariantRelation};
use crate::library_db;
use crate::system_shard::SystemGame;

#[derive(Clone, Debug)]
pub(crate) struct MachineFamilyCandidate {
    pub(crate) game: SystemGame,
    pub(crate) identity_id: String,
    pub(crate) family_id: String,
    pub(crate) relation: FastFiveVariantRelation,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MachineFamilyProjection {
    pub(crate) games: Vec<SystemGame>,
    pub(crate) variants: Vec<FastFiveGameVariant>,
}

pub(crate) fn project_machine_families(
    mut candidates: Vec<MachineFamilyCandidate>,
) -> MachineFamilyProjection {
    // Deduplicate launch references before assigning families.  Sorting by
    // stable key makes the retained row deterministic even when two profiles
    // discover the same path in a different order.
    candidates.sort_unstable_by(|left, right| {
        left.game
            .launch_ref
            .cmp(&right.game.launch_ref)
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    candidates.dedup_by(|left, right| left.game.launch_ref == right.game.launch_ref);

    let mut prepared = candidates
        .into_iter()
        .map(|candidate| {
            let family = family_key(&candidate);
            let is_parent = !candidate.identity_id.is_empty()
                && candidate.identity_id.eq_ignore_ascii_case(&family);
            (family, is_parent, candidate)
        })
        .collect::<Vec<_>>();
    prepared.sort_unstable_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| {
                left.2
                    .game
                    .title
                    .to_ascii_lowercase()
                    .cmp(&right.2.game.title.to_ascii_lowercase())
            })
            .then_with(|| left.2.game.launch_ref.cmp(&right.2.game.launch_ref))
            .then_with(|| left.2.game.stable_key.cmp(&right.2.game.stable_key))
    });

    let mut projection = MachineFamilyProjection {
        games: Vec::new(),
        variants: Vec::new(),
    };
    let mut position = 0;
    while position < prepared.len() {
        let family = prepared[position].0.clone();
        let end = prepared[position..]
            .iter()
            .position(|candidate| candidate.0 != family)
            .map_or(prepared.len(), |offset| position + offset);
        let head = prepared[position].2.game.clone();
        let family_stable_key = head.stable_key.clone();
        projection.games.push(head);
        for (_, _, candidate) in prepared[position + 1..end].iter() {
            projection.variants.push(FastFiveGameVariant {
                family_stable_key: family_stable_key.clone(),
                relation: candidate.relation,
                game: candidate.game.clone(),
            });
        }
        position = end;
    }
    projection.games.sort_unstable_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.stable_key.cmp(&right.stable_key))
    });
    projection.variants.sort_unstable_by(|left, right| {
        left.family_stable_key
            .cmp(&right.family_stable_key)
            .then_with(|| left.game.launch_ref.cmp(&right.game.launch_ref))
            .then_with(|| left.game.stable_key.cmp(&right.game.stable_key))
    });
    projection
}

fn family_key(candidate: &MachineFamilyCandidate) -> String {
    let family = library_db::normalize_id(&candidate.family_id);
    if !candidate.family_id.trim().is_empty() && family != "unknown" {
        return family;
    }
    format!("launch:{}", candidate.game.launch_ref.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, launch_ref: &str) -> SystemGame {
        SystemGame {
            stable_key: format!("arcade\u{1f}{title}\u{1f}{launch_ref}"),
            title: title.to_string(),
            launch_ref: launch_ref.to_string(),
            preview_archive_path: String::new(),
            preview_asset_key: String::new(),
            has_preview: false,
            year: None,
            manufacturer: String::new(),
            category: String::new(),
            players: None,
            control: String::new(),
            is_new: false,
            launch_plan: None,
        }
    }

    #[test]
    fn projects_parent_first_and_keeps_unresolved_rows_standalone() {
        let projection = project_machine_families(vec![
            MachineFamilyCandidate {
                game: row("Example (Japan)", "japan.mra"),
                identity_id: "examplej".to_string(),
                family_id: "example".to_string(),
                relation: FastFiveVariantRelation::ArcadeVariant,
            },
            MachineFamilyCandidate {
                game: row("Example", "example.mra"),
                identity_id: "example".to_string(),
                family_id: "example".to_string(),
                relation: FastFiveVariantRelation::ArcadeVariant,
            },
            MachineFamilyCandidate {
                game: row("Unknown", "unknown.mra"),
                identity_id: String::new(),
                family_id: String::new(),
                relation: FastFiveVariantRelation::ArcadeVariant,
            },
        ]);
        assert_eq!(projection.games.len(), 2);
        assert_eq!(projection.variants.len(), 1);
        assert_eq!(projection.games[0].title, "Example");
        assert_eq!(
            projection.variants[0].family_stable_key,
            projection.games[0].stable_key
        );
    }

    #[test]
    fn deduplicates_launch_references_deterministically() {
        let projection = project_machine_families(vec![
            MachineFamilyCandidate {
                game: row("Z", "same.zip"),
                identity_id: "z".to_string(),
                family_id: "z".to_string(),
                relation: FastFiveVariantRelation::NeoGeoVariant,
            },
            MachineFamilyCandidate {
                game: row("A", "same.zip"),
                identity_id: "a".to_string(),
                family_id: "a".to_string(),
                relation: FastFiveVariantRelation::NeoGeoVariant,
            },
        ]);
        assert_eq!(projection.games.len(), 1);
        assert_eq!(projection.games[0].title, "A");
    }
}
