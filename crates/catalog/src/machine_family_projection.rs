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

struct PreparedCandidate {
    family: String,
    is_parent: bool,
    normalized_title: String,
    candidate: MachineFamilyCandidate,
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
            let normalized_title = candidate.game.title.to_ascii_lowercase();
            PreparedCandidate {
                family,
                is_parent,
                normalized_title,
                candidate,
            }
        })
        .collect::<Vec<_>>();
    prepared.sort_unstable_by(|left, right| {
        left.family
            .cmp(&right.family)
            .then_with(|| right.is_parent.cmp(&left.is_parent))
            .then_with(|| left.normalized_title.cmp(&right.normalized_title))
            .then_with(|| {
                left.candidate
                    .game
                    .launch_ref
                    .cmp(&right.candidate.game.launch_ref)
            })
            .then_with(|| {
                left.candidate
                    .game
                    .stable_key
                    .cmp(&right.candidate.game.stable_key)
            })
    });

    let mut projection = MachineFamilyProjection {
        games: Vec::new(),
        variants: Vec::new(),
    };
    let mut prepared = prepared.into_iter().peekable();
    while let Some(head) = prepared.next() {
        let family = head.family.clone();
        let family_stable_key = head.candidate.game.stable_key.clone();
        projection.games.push(head.candidate.game);
        while prepared
            .peek()
            .is_some_and(|candidate| candidate.family == family)
        {
            let variant = prepared.next().expect("peeked family candidate");
            projection.variants.push(FastFiveGameVariant {
                family_stable_key: family_stable_key.clone(),
                relation: variant.candidate.relation,
                game: variant.candidate.game,
            });
        }
    }
    projection.games.sort_unstable_by_cached_key(|game| {
        (game.title.to_ascii_lowercase(), game.stable_key.clone())
    });
    projection.variants.sort_unstable_by_cached_key(|variant| {
        (
            variant.family_stable_key.clone(),
            variant.game.launch_ref.clone(),
            variant.game.stable_key.clone(),
        )
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

    #[test]
    fn shuffled_candidates_have_identical_projection_and_absent_parent_is_standalone() {
        let candidates = vec![
            MachineFamilyCandidate {
                game: row("Clone B", "b.mra"),
                identity_id: "cloneb".to_string(),
                family_id: "parent".to_string(),
                relation: FastFiveVariantRelation::ArcadeVariant,
            },
            MachineFamilyCandidate {
                game: row("Clone A", "a.mra"),
                identity_id: "clonea".to_string(),
                family_id: "parent".to_string(),
                relation: FastFiveVariantRelation::ArcadeVariant,
            },
            MachineFamilyCandidate {
                game: row("Other", "other.mra"),
                identity_id: "other".to_string(),
                family_id: "missing-parent".to_string(),
                relation: FastFiveVariantRelation::ArcadeVariant,
            },
        ];
        let mut shuffled = candidates.clone();
        shuffled.reverse();
        let first = project_machine_families(candidates);
        let second = project_machine_families(shuffled);
        assert_eq!(
            first
                .games
                .iter()
                .map(|game| (&game.title, &game.launch_ref))
                .collect::<Vec<_>>(),
            second
                .games
                .iter()
                .map(|game| (&game.title, &game.launch_ref))
                .collect::<Vec<_>>()
        );
        assert_eq!(first.variants.len(), 2);
        assert!(first.variants.iter().all(|variant| {
            first
                .games
                .iter()
                .any(|game| game.stable_key == variant.family_stable_key)
        }));
    }
}
