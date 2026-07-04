//! Catalog coverage audit for Main-derived core/game surfaces.
//!
//! The audit records launchable-looking things the catalog scanner did not turn
//! into games. These rows are diagnostics and stamp inputs, not launch entries.

use crate::catalog_discovery;
use crate::launch_profiles::{self, LaunchProfile, PayloadDisposition};
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

pub(crate) fn audit_catalog_coverage(
    roots: &[String],
    profiles: &[LaunchProfile],
) -> Vec<CatalogAuditRow> {
    let mut rows = BTreeMap::<String, CatalogAuditRow>::new();
    audit_installed_cores(roots, profiles, &mut rows);
    audit_game_directories(roots, profiles, &mut rows);
    rows.into_values().collect()
}

fn audit_installed_cores(
    roots: &[String],
    profiles: &[LaunchProfile],
    rows: &mut BTreeMap<String, CatalogAuditRow>,
) {
    for core in catalog_discovery::installed_cores_for_roots(roots) {
        let core_id = core.core_id;
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
    roots: &[String],
    profiles: &[LaunchProfile],
    rows: &mut BTreeMap<String, CatalogAuditRow>,
) {
    let cataloged_dirs = cataloged_game_dirs(profiles);
    for fact in catalog_discovery::top_level_game_dirs_for_roots(roots) {
        let name = fact.name.as_str();
        let path = &fact.path;
        let key = name.to_ascii_lowercase();
        if cataloged_dirs.contains(&key) {
            audit_non_indexed_zips_in_cataloged_dir(path, name, profiles, rows);
            continue;
        }
        if !fact.has_payloadish_files() {
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
                    reason: "known-game-dir-without-installed-core".to_string(),
                },
            );
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

fn audit_non_indexed_zips_in_cataloged_dir(
    dir: &Path,
    dir_name: &str,
    profiles: &[LaunchProfile],
    rows: &mut BTreeMap<String, CatalogAuditRow>,
) {
    let Some(profile) = profiles.iter().find(|profile| {
        profile
            .game_dirs
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(dir_name))
    }) else {
        return;
    };
    if !profile.archive_entry_rules.is_empty() || !profile.collection_rules.is_empty() {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() || should_ignore_hidden_path(&path) || !path_ext_eq(&path, "zip") {
            continue;
        }
        insert_audit_row(
            rows,
            CatalogAuditRow {
                core_id: profile.core_name.to_string(),
                core_path: profile.core_path.as_deref().unwrap_or_default().to_string(),
                expected_game_dir: format!("games/{dir_name}"),
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

fn path_ext_eq(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

fn should_ignore_hidden_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.len() > 1 && name.starts_with('.')
    })
}

fn source_name(kind: crate::launch_profiles::RuleSourceKind) -> &'static str {
    match kind {
        crate::launch_profiles::RuleSourceKind::MainSource => "main-derived",
        crate::launch_profiles::RuleSourceKind::Mgl => "mgl",
        crate::launch_profiles::RuleSourceKind::Mra => "mra",
        crate::launch_profiles::RuleSourceKind::ConfStr => "main-derived",
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
            &dir.join("MiSTer MagiK Additions - ChannelF.zip"),
            &[("Alien Invasion.chf", b"rom")],
        );

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(rows.iter().any(|row| {
            row.expected_game_dir == "games/ChannelF"
                && row.catalog_status == "uncataloged"
                && row.reason == "game-dir-only-has-unindexed-zip-archives"
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn known_profile_without_zip_entry_rules_reports_zip_as_unsupported() {
        let root = unique_temp_dir("audit-psx-zip");
        let dir = root.join("games/PSX");
        std::fs::create_dir_all(&dir).expect("create psx dir");
        write_stored_zip(
            &dir.join("MiSTer MagiK Additions - PSX.zip"),
            &[("Game.cue", b"cue")],
        );

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
    fn appledouble_zip_sidecars_are_not_reported() {
        let root = unique_temp_dir("audit-appledouble-zip");
        let dir = root.join("games/SMS");
        std::fs::create_dir_all(&dir).expect("create sms dir");
        write_stored_zip(
            &dir.join("._MiSTer MagiK Additions - SMS.zip"),
            &[("Game.sms", b"rom")],
        );

        let rows = audit_catalog_coverage(
            &[root.display().to_string()],
            &launch_profiles::builtin_profiles(),
        );

        assert!(!rows
            .iter()
            .any(|row| { row.reason.contains("._MiSTer MagiK Additions - SMS.zip") }));
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
