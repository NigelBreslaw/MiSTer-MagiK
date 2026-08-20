// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalog coverage audit for Main-derived core/game surfaces.
//!
//! The audit records launchable-looking things the catalog scanner did not turn
//! into games. These rows are diagnostics and stamp inputs, not launch entries.

use crate::catalog_discovery;
use crate::launch_profiles::{
    self, LaunchProfile, PayloadDisposition, RuleSourceKind, RuntimeProfileDecision,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogAuditRow {
    pub core_id: String,
    pub core_path: String,
    pub expected_game_dir: String,
    pub extensions: String,
    pub mount_kind: String,
    pub source: String,
    pub catalog_status: String,
    pub reason: String,
}

impl CatalogAuditRow {
    pub fn evidence_source(&self) -> &str {
        match self.source.as_str() {
            "mgl" => "mgl-setname",
            "main-derived" | "main-source" => "core-mount-contract",
            "runtime-discovered" if self.reason == "matched-catalog-profile" => {
                "normalized-name-or-descriptor"
            }
            "runtime-discovered" => "runtime-observation",
            _ => "unknown",
        }
    }

    pub fn evidence_confidence(&self) -> &str {
        match self.evidence_source() {
            "mgl-setname" | "core-mount-contract" => "authoritative",
            "normalized-name-or-descriptor" => "strong",
            "runtime-observation" => "weak",
            _ => "none",
        }
    }

    pub fn content_role(&self) -> &str {
        match self.catalog_status.as_str() {
            "cataloged" => "playable-candidate",
            "support-only" => "support",
            _ => "unknown",
        }
    }

    pub fn suppression_reason(&self) -> &str {
        if self.catalog_status == "cataloged" {
            ""
        } else {
            &self.reason
        }
    }
}

#[cfg(test)]
pub(crate) fn audit_catalog_coverage(
    roots: &[String],
    profiles: &[LaunchProfile],
) -> Vec<CatalogAuditRow> {
    let installed_cores = catalog_discovery::installed_cores_for_roots(roots);
    let game_dirs = catalog_discovery::top_level_game_dirs_for_roots(roots);
    audit_catalog_coverage_from_facts(roots, profiles, &installed_cores, &game_dirs)
}

pub(crate) fn audit_catalog_coverage_from_facts(
    roots: &[String],
    profiles: &[LaunchProfile],
    installed_cores: &[catalog_discovery::InstalledCore],
    game_dirs: &[catalog_discovery::GameDirFact],
) -> Vec<CatalogAuditRow> {
    let mut rows = BTreeMap::<String, CatalogAuditRow>::new();
    audit_installed_cores(installed_cores, profiles, &mut rows);
    audit_game_directories(game_dirs, installed_cores, profiles, &mut rows);
    audit_prepared_collections(roots, &mut rows);
    rows.into_values().collect()
}

fn audit_prepared_collections(roots: &[String], rows: &mut BTreeMap<String, CatalogAuditRow>) {
    let neon68k_payload_present = roots.iter().any(|root| {
        crate::prepared_collections::neon68k_payload_signature_for_library_root(Path::new(root))
            .is_some_and(|path| path.is_file())
    });
    if !neon68k_payload_present {
        return;
    }
    let neon68k_launcher_available = roots.iter().any(|root| {
        crate::prepared_collections::neon68k_launcher_roots_for_library_root(Path::new(root))
            .into_iter()
            .any(|path| crate::prepared_collections::neon68k_launcher_root_is_available(&path))
    });
    if neon68k_launcher_available {
        return;
    }
    insert_audit_row(
        rows,
        CatalogAuditRow {
            core_id: "X68000".to_string(),
            core_path: "_Computer/X68000".to_string(),
            expected_game_dir: "_Computer/_X68000 Games".to_string(),
            extensions: "mgl".to_string(),
            mount_kind: "mgl".to_string(),
            source: "prepared-collection".to_string(),
            catalog_status: "uncataloged".to_string(),
            reason: "neon68k-launcher-root-missing-or-unreadable".to_string(),
        },
    );
}

fn audit_installed_cores(
    installed_cores: &[catalog_discovery::InstalledCore],
    profiles: &[LaunchProfile],
    rows: &mut BTreeMap<String, CatalogAuditRow>,
) {
    for (index, core) in installed_cores.iter().enumerate() {
        if index.is_multiple_of(16) {
            crate::cooperative_work::checkpoint();
        }
        let core_id = core.core_id.clone();
        let game_dir = main_default_game_dir_for_core(&core_id);
        let profile = profile_for_core_or_dir(profiles, &core_id, &game_dir);
        let row = match profile {
            Some(profile) => cataloged_row_for_profile(
                profile,
                core.path.display().to_string(),
                format!("games/{game_dir}"),
            ),
            None => CatalogAuditRow {
                core_id: core_id.clone(),
                core_path: core.path.display().to_string(),
                expected_game_dir: format!("games/{game_dir}"),
                extensions: inferred_extensions_for_game_dir(&game_dir),
                mount_kind: "load-file".to_string(),
                source: "main-derived".to_string(),
                catalog_status: "uncataloged".to_string(),
                reason: "installed-core-has-no-catalog-profile".to_string(),
            },
        };
        insert_audit_row(rows, row);
    }
}

fn audit_game_directories(
    game_dirs: &[catalog_discovery::GameDirFact],
    installed_cores: &[catalog_discovery::InstalledCore],
    profiles: &[LaunchProfile],
    rows: &mut BTreeMap<String, CatalogAuditRow>,
) {
    let cataloged_dirs = cataloged_game_dirs(profiles);
    let runtime_plans = launch_profiles::runtime_profile_plans_for_game_dirs_with_cores(
        game_dirs,
        installed_cores,
        &BTreeSet::new(),
    )
    .into_iter()
    .map(|plan| (plan.game_dir_name.to_ascii_lowercase(), plan.decision))
    .collect::<BTreeMap<_, _>>();
    for (index, fact) in game_dirs.iter().enumerate() {
        if index.is_multiple_of(16) {
            crate::cooperative_work::checkpoint();
        }
        let name = fact.name.as_str();
        let key = name.to_ascii_lowercase();
        if cataloged_dirs.contains(&key) {
            if let Some(profile) = launch_profiles::profile_for_game_dir(profiles, name)
                && profile.provenance.kind == RuleSourceKind::ConfStr
            {
                insert_audit_row(
                    rows,
                    cataloged_row_for_profile(
                        profile,
                        profile.core_path.as_deref().unwrap_or_default().to_string(),
                        format!("games/{name}"),
                    ),
                );
            }
            if fact.has_zip_files {
                audit_non_indexed_zips_in_cataloged_dir(fact, profiles, rows);
            }
            continue;
        }
        if let Some(profile) = launch_profiles::generic_manifest_profile_for_game_dir(name) {
            insert_audit_row(
                rows,
                CatalogAuditRow {
                    core_id: profile.core_name.to_string(),
                    core_path: profile.core_path.as_deref().unwrap_or_default().to_string(),
                    expected_game_dir: format!("games/{name}"),
                    extensions: profile_payload_extensions(&profile),
                    mount_kind: "load-file".to_string(),
                    source: source_name(profile.provenance.kind).to_string(),
                    catalog_status: "support-only".to_string(),
                    reason: "no-installed-core".to_string(),
                },
            );
            continue;
        }
        if let Some(decision) = runtime_plans.get(&key) {
            insert_audit_row(rows, audit_row_for_runtime_decision(fact, decision));
            continue;
        }
        insert_audit_row(
            rows,
            CatalogAuditRow {
                core_id: name.to_string(),
                core_path: String::new(),
                expected_game_dir: format!("games/{name}"),
                extensions: inferred_extensions_for_game_dir(name),
                mount_kind: "load-file".to_string(),
                source: "unknown".to_string(),
                catalog_status: "uncataloged".to_string(),
                reason: if fact.has_zip_files && !fact.has_payload_files {
                    "game-dir-only-has-unindexed-zip-archives".to_string()
                } else {
                    "game-dir-has-no-catalog-profile".to_string()
                },
            },
        );
    }
}

fn audit_row_for_runtime_decision(
    fact: &catalog_discovery::GameDirFact,
    decision: &RuntimeProfileDecision,
) -> CatalogAuditRow {
    let (core_id, catalog_status, reason) = match decision {
        RuntimeProfileDecision::Catalogable { profile } => (
            profile.core_name.clone(),
            "cataloged".to_string(),
            "matched-catalog-profile".to_string(),
        ),
        RuntimeProfileDecision::NoInstalledCore => (
            fact.name.clone(),
            "uncataloged".to_string(),
            "no-installed-core".to_string(),
        ),
        RuntimeProfileDecision::EmptyOrMediaOnly => (
            fact.name.clone(),
            "support-only".to_string(),
            "no-valid-games".to_string(),
        ),
        RuntimeProfileDecision::NoKnownPayloadExtension => (
            fact.name.clone(),
            "uncataloged".to_string(),
            "unsupported-extension".to_string(),
        ),
        RuntimeProfileDecision::Ambiguous { core_ids } => (
            core_ids.join(","),
            "uncataloged".to_string(),
            "ambiguous-alias".to_string(),
        ),
    };
    CatalogAuditRow {
        core_id,
        core_path: String::new(),
        expected_game_dir: format!("games/{}", fact.name),
        extensions: observed_or_inferred_extensions(fact),
        mount_kind: "load-file".to_string(),
        source: "runtime-discovered".to_string(),
        catalog_status,
        reason,
    }
}

fn audit_non_indexed_zips_in_cataloged_dir(
    fact: &catalog_discovery::GameDirFact,
    profiles: &[LaunchProfile],
    rows: &mut BTreeMap<String, CatalogAuditRow>,
) {
    let Some(profile) = profiles.iter().find(|profile| {
        profile
            .game_dirs
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&fact.name))
    }) else {
        return;
    };
    if !profile.archive_entry_rules.is_empty() || !profile.collection_rules.is_empty() {
        return;
    }
    for path in &fact.direct_zip_paths {
        insert_audit_row(
            rows,
            CatalogAuditRow {
                core_id: profile.core_name.to_string(),
                core_path: profile.core_path.as_deref().unwrap_or_default().to_string(),
                expected_game_dir: format!("games/{}", fact.name),
                extensions: profile_payload_extensions(profile),
                mount_kind: "load-file".to_string(),
                source: "main-derived".to_string(),
                catalog_status: "unsupported".to_string(),
                reason: format!("zip-archive-not-indexed:{}", path.display()),
            },
        );
    }
}

fn insert_audit_row(rows: &mut BTreeMap<String, CatalogAuditRow>, row: CatalogAuditRow) {
    let key = format!(
        "{}\t{}\t{}\t{}",
        row.catalog_status, row.expected_game_dir, row.core_id, row.reason
    );
    rows.entry(key).or_insert(row);
}

fn cataloged_row_for_profile(
    profile: &LaunchProfile,
    core_path: String,
    expected_game_dir: String,
) -> CatalogAuditRow {
    CatalogAuditRow {
        core_id: profile.core_name.to_string(),
        core_path,
        expected_game_dir,
        extensions: profile_payload_extensions(profile),
        mount_kind: "load-file".to_string(),
        source: source_name(profile.provenance.kind).to_string(),
        catalog_status: "cataloged".to_string(),
        reason: "matched-catalog-profile".to_string(),
    }
}

fn profile_payload_extensions(profile: &LaunchProfile) -> String {
    let mut extensions = BTreeSet::new();
    for rule in &profile.payload_rules {
        if rule.disposition == PayloadDisposition::Playable {
            for ext in &rule.extensions {
                extensions.insert(ext.to_ascii_lowercase());
            }
        }
    }
    extensions.into_iter().collect::<Vec<_>>().join(",")
}

fn observed_or_inferred_extensions(fact: &catalog_discovery::GameDirFact) -> String {
    if fact.payload_extensions.is_empty() {
        inferred_extensions_for_game_dir(&fact.name)
    } else {
        fact.payload_extensions
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn cataloged_game_dirs(profiles: &[LaunchProfile]) -> BTreeSet<String> {
    profiles
        .iter()
        .flat_map(|profile| profile.game_dirs.iter())
        .filter(|dir| !dir.starts_with('_'))
        .map(|dir| dir.to_ascii_lowercase())
        .collect()
}

fn profile_for_core_or_dir<'a>(
    profiles: &'a [LaunchProfile],
    core_id: &str,
    game_dir: &str,
) -> Option<&'a LaunchProfile> {
    profiles.iter().find(|profile| {
        profile.core_name.eq_ignore_ascii_case(core_id)
            || profile
                .game_dirs
                .iter()
                .any(|dir| dir.eq_ignore_ascii_case(game_dir))
    })
}

fn main_default_game_dir_for_core(core_id: &str) -> String {
    if core_id.eq_ignore_ascii_case("minimig") {
        "Amiga".to_string()
    } else {
        core_id.to_string()
    }
}

fn inferred_extensions_for_game_dir(game_dir: &str) -> String {
    let exts = match game_dir.to_ascii_lowercase().as_str() {
        "atari2600" | "atari2600-sinden" => &["a26", "bin"][..],
        "atari5200" => &["a52", "bin"],
        "atari7800" => &["a78", "bin"],
        "atarilynx" => &["lnx"],
        "coleco" => &["col", "rom"],
        "fds" => &["fds"],
        "gameboy" | "gameboy2p" => &["gb"],
        "intellivision" => &["int", "rom", "bin"],
        "ngpc" => &["ngc"],
        "neogeopocket" => &["ngp"],
        "s32x" => &["32x"],
        "sgb2" => &["sfc", "smc"],
        "satellaview" => &["sfc", "smc", "bs"],
        "supergrafx" | "tgfx16" => &["pce"],
        "vectrex" => &["vec"],
        "wonderswan" => &["ws"],
        "wonderswancolor" => &["wsc"],
        _ => &[],
    };
    exts.join(",")
}

fn source_name(kind: crate::launch_profiles::RuleSourceKind) -> &'static str {
    match kind {
        crate::launch_profiles::RuleSourceKind::MainSource => "main-derived",
        crate::launch_profiles::RuleSourceKind::Mgl => "mgl",
        crate::launch_profiles::RuleSourceKind::Mra => "mra",
        crate::launch_profiles::RuleSourceKind::ConfStr => "runtime-discovered",
        crate::launch_profiles::RuleSourceKind::MagikProfile => "magik-special",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch_profiles;
    use crate::test_support::{unique_temp_dir, write_stored_zip};

    #[test]
    fn unknown_collection_zip_is_reported_as_uncataloged() {
        let root = unique_temp_dir("audit-unknown-zip");
        let dir = root.join("games/ChannelF");
        std::fs::create_dir_all(&dir).expect("create unknown dir");
        write_stored_zip(
            &dir.join("Packed ChannelF Games.zip"),
            &[("Alien Invasion.chf", b"rom")],
        );

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "games/ChannelF"
                && row.catalog_status == "uncataloged"
                && row.reason == "no-installed-core"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn known_profile_without_zip_entry_rules_reports_zip_as_unsupported() {
        let root = unique_temp_dir("audit-psx-zip");
        let dir = root.join("games/PSX");
        std::fs::create_dir_all(&dir).expect("create psx dir");
        write_stored_zip(&dir.join("Packed PSX Games.zip"), &[("Game.cue", b"cue")]);

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "games/PSX"
                && row.catalog_status == "unsupported"
                && row.reason.contains("zip-archive-not-indexed")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deferred_zip_audit_uses_primary_walk_generation_without_reopening_directory() {
        let root = unique_temp_dir("audit-retained-psx-zip");
        let dir = root.join("games/PSX");
        let zip = dir.join("Packed PSX Games.zip");
        std::fs::create_dir_all(&dir).expect("create psx dir");
        write_stored_zip(&zip, &[("Game.cue", b"cue")]);
        let roots = vec![root.display().to_string()];
        let game_dirs = catalog_discovery::top_level_game_dirs_for_roots(&roots);
        let installed_cores = catalog_discovery::installed_cores_for_roots(&roots);
        assert_eq!(game_dirs[0].direct_zip_paths, vec![zip.clone()]);

        std::fs::remove_file(&zip).expect("remove zip after primary walk");
        let rows = audit_catalog_coverage_from_facts(
            &roots,
            &launch_profiles::builtin_profiles(),
            &installed_cores,
            &game_dirs,
        );

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "games/PSX"
                && row.catalog_status == "unsupported"
                && row.reason == format!("zip-archive-not-indexed:{}", zip.display())
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn appledouble_zip_sidecars_are_not_reported() {
        let root = unique_temp_dir("audit-appledouble-zip");
        let dir = root.join("games/SMS");
        std::fs::create_dir_all(&dir).expect("create sms dir");
        write_stored_zip(&dir.join("._Packed SMS Games.zip"), &[("Game.sms", b"rom")]);

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(
            !rows
                .iter()
                .any(|row| { row.reason.contains("._Packed SMS Games.zip") })
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn neon68k_boot_volume_without_launchers_is_actionable() {
        let root = unique_temp_dir("audit-neon68k-missing-launchers");
        let payload_dir = root.join("games/X68000");
        std::fs::create_dir_all(&payload_dir).expect("create X68000 payload dir");
        std::fs::write(payload_dir.join("boot3.vhd"), b"boot").expect("write Neon68K boot");

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "_Computer/_X68000 Games"
                && row.catalog_status == "uncataloged"
                && row.reason == "neon68k-launcher-root-missing-or-unreadable"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn neon68k_followable_launcher_symlink_satisfies_audit() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("audit-neon68k-linked-launchers");
        let payload_dir = root.join("games/X68000");
        let launcher_source = root.join("launcher-source");
        std::fs::create_dir_all(&payload_dir).expect("create X68000 payload dir");
        std::fs::create_dir_all(&launcher_source).expect("create launcher source");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::write(payload_dir.join("boot3.vhd"), b"boot").expect("write Neon68K boot");
        symlink(&launcher_source, root.join("_Computer/_X68000 Games"))
            .expect("create launcher symlink");

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(
            !rows
                .iter()
                .any(|row| { row.reason == "neon68k-launcher-root-missing-or-unreadable" })
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn media_only_helper_dirs_are_not_reported_as_uncataloged_game_dirs() {
        let root = unique_temp_dir("audit-media-only-helper-dirs");
        let channel_f = root.join("games/ChannelF");
        std::fs::create_dir_all(channel_f.join("screenshot-magik")).expect("create screenshots");
        std::fs::create_dir_all(channel_f.join("BoxArt")).expect("create boxart");
        std::fs::write(channel_f.join("screenshot-magik/Fake.chf"), b"media")
            .expect("write fake media payload");
        write_stored_zip(
            &channel_f.join("BoxArt/Fake.zip"),
            &[("Fake.chf", b"media")],
        );

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(!rows.iter().any(|row| {
            row.expected_game_dir == "games/ChannelF" && row.catalog_status == "uncataloged"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installed_unknown_core_changes_audit_surface() {
        let root = unique_temp_dir("audit-new-core");
        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console dir");
        std::fs::write(console.join("ChannelF_20260629.rbf"), b"rbf").expect("write core");

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(rows.iter().any(|row| {
            row.core_id == "ChannelF"
                && row.expected_game_dir == "games/ChannelF"
                && row.catalog_status == "uncataloged"
                && row.reason == "installed-core-has-no-catalog-profile"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_game_dir_without_core_reports_no_installed_core() {
        let root = unique_temp_dir("audit-runtime-no-core");
        let gameboy = root.join("games/Gameboy");
        std::fs::create_dir_all(&gameboy).expect("create gameboy dir");
        std::fs::write(gameboy.join("Tetris.gb"), b"rom").expect("write rom");
        let roots = vec![root.display().to_string()];
        let profiles = launch_profiles::active_profiles_for_roots(&roots);

        let rows = audit_catalog_coverage(&roots, &profiles);

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "games/Gameboy"
                && row.source == "runtime-discovered"
                && row.catalog_status == "uncataloged"
                && row.reason == "no-installed-core"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_empty_game_dir_reports_no_valid_games() {
        let root = unique_temp_dir("audit-runtime-empty");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Gameboy")).expect("create gameboy dir");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");
        let roots = vec![root.display().to_string()];
        let profiles = launch_profiles::active_profiles_for_roots(&roots);

        let rows = audit_catalog_coverage(&roots, &profiles);

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "games/Gameboy"
                && row.source == "runtime-discovered"
                && row.catalog_status == "support-only"
                && row.reason == "no-valid-games"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_exact_core_extension_derivation_reports_cataloged() {
        let root = unique_temp_dir("audit-runtime-derived-extension");
        std::fs::create_dir_all(root.join("_Computer")).expect("create computer dir");
        std::fs::create_dir_all(root.join("games/C64")).expect("create c64 dir");
        std::fs::write(root.join("_Computer/C64_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/C64/Impossible Mission.d64"), b"disk").expect("write disk");
        let roots = vec![root.display().to_string()];
        let profiles = launch_profiles::active_profiles_for_roots(&roots);

        let rows = audit_catalog_coverage(&roots, &profiles);

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "games/C64"
                && row.extensions == "d64"
                && row.source == "runtime-discovered"
                && row.catalog_status == "cataloged"
                && row.reason == "matched-catalog-profile"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_ambiguous_alias_reports_stable_reason() {
        let root = unique_temp_dir("audit-runtime-ambiguous-alias");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Loose")).expect("create loose dir");
        std::fs::write(root.join("_Console/ColecoVision_20260630.rbf"), b"rbf")
            .expect("write coleco core");
        std::fs::write(root.join("_Console/SMS_20260630.rbf"), b"rbf").expect("write sms core");
        std::fs::write(root.join("games/Loose/Zaxxon.sg"), b"rom").expect("write rom");
        let roots = vec![root.display().to_string()];
        let profiles = launch_profiles::active_profiles_for_roots(&roots);

        let rows = audit_catalog_coverage(&roots, &profiles);

        assert!(rows.iter().any(|row| {
            row.core_id == "ColecoVision,SMS"
                && row.expected_game_dir == "games/Loose"
                && row.extensions == "sg"
                && row.source == "runtime-discovered"
                && row.catalog_status == "uncataloged"
                && row.reason == "ambiguous-alias"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_cataloged_game_dir_reports_runtime_source() {
        let root = unique_temp_dir("audit-runtime-cataloged");
        std::fs::create_dir_all(root.join("_Console")).expect("create console dir");
        std::fs::create_dir_all(root.join("games/Gameboy")).expect("create gameboy dir");
        std::fs::write(root.join("_Console/Gameboy_20260630.rbf"), b"rbf").expect("write core");
        std::fs::write(root.join("games/Gameboy/Tetris.gb"), b"rom").expect("write rom");
        let roots = vec![root.display().to_string()];
        let profiles = launch_profiles::active_profiles_for_roots(&roots);

        let rows = audit_catalog_coverage(&roots, &profiles);

        assert!(rows.iter().any(|row| {
            row.core_id == "Gameboy"
                && row.expected_game_dir == "games/Gameboy"
                && row.source == "runtime-discovered"
                && row.catalog_status == "cataloged"
                && row.reason == "matched-catalog-profile"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn appledouble_core_sidecars_are_not_reported_as_installed_cores() {
        let root = unique_temp_dir("audit-appledouble-core");
        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console dir");
        std::fs::write(console.join("._WonderSwanColor_20260629.rbf"), b"sidecar")
            .expect("write sidecar core");

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(!rows.iter().any(|row| row.core_id == "._WonderSwanColor"));
        let _ = std::fs::remove_dir_all(root);
    }
}
