//! Cheap discovery checkpoint for warm catalog drift detection.
//!
//! The checkpoint records stable, low-cost catalog inputs. It is intentionally
//! not a full file manifest: warm validation should notice new systems/cores
//! without walking every game payload.

use crate::catalog_config::{CATALOG_BUILD_VERSION, SCHEMA_VERSION};
use crate::catalog_scan::should_ignore_path;
use crate::core_audit::CatalogAuditRow;
use crate::launch_profiles::{
    self, core_launch_manifest_fingerprint, CORE_LAUNCH_MANIFEST_VERSION, PROFILE_SET_VERSION,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

const CHECKPOINT_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const CHECKPOINT_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDiscoveryCheckpoint {
    lines: Vec<String>,
}

impl CatalogDiscoveryCheckpoint {
    pub fn from_lines(lines: Vec<String>) -> Self {
        Self { lines }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hash = CHECKPOINT_HASH_OFFSET;
        for line in &self.lines {
            hash = fnv64_update(hash, line.as_bytes());
            hash = fnv64_update(hash, &[0xff]);
        }
        hash
    }

    pub fn fingerprint_hex(&self) -> String {
        format!("{:016x}", self.fingerprint())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogDriftSummary {
    pub unchanged: bool,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub changed_roots: usize,
    pub changed_core_roots: usize,
    pub changed_cores: usize,
    pub changed_game_dirs: usize,
    pub changed_audit_rows: usize,
    pub changed_metadata: usize,
    pub changed_versions: usize,
    pub detail: String,
}

impl CatalogDriftSummary {
    pub fn unchanged() -> Self {
        Self {
            unchanged: true,
            detail: "checkpoint unchanged".to_string(),
            ..Self::default()
        }
    }

    pub fn from_checkpoints(
        stored: Option<&CatalogDiscoveryCheckpoint>,
        current: &CatalogDiscoveryCheckpoint,
    ) -> Self {
        let Some(stored) = stored else {
            return Self {
                unchanged: false,
                added_lines: current.lines.len(),
                detail: "checkpoint missing".to_string(),
                ..Self::default()
            };
        };
        if stored == current {
            return Self::unchanged();
        }

        let stored_set = stored.lines.iter().collect::<BTreeSet<_>>();
        let current_set = current.lines.iter().collect::<BTreeSet<_>>();
        let mut summary = Self {
            unchanged: false,
            added_lines: current_set.difference(&stored_set).count(),
            removed_lines: stored_set.difference(&current_set).count(),
            ..Self::default()
        };
        for line in current_set
            .difference(&stored_set)
            .chain(stored_set.difference(&current_set))
        {
            match line.split('\t').next().unwrap_or_default() {
                "schema" | "catalog-build" | "profile-set" | "core-launch-manifest" => {
                    summary.changed_versions += 1
                }
                "root" => summary.changed_roots += 1,
                "core-search-root" => summary.changed_core_roots += 1,
                "installed-core" => summary.changed_cores += 1,
                "game-dir" => summary.changed_game_dirs += 1,
                "core-audit-row" | "core-audit-summary" => summary.changed_audit_rows += 1,
                "mame-metadata" | "hbmame-metadata" => summary.changed_metadata += 1,
                _ => {}
            }
        }
        summary.detail = format!(
            "checkpoint changed added={} removed={} versions={} roots={} core_roots={} cores={} game_dirs={} audit={} metadata={}",
            summary.added_lines,
            summary.removed_lines,
            summary.changed_versions,
            summary.changed_roots,
            summary.changed_core_roots,
            summary.changed_cores,
            summary.changed_game_dirs,
            summary.changed_audit_rows,
            summary.changed_metadata
        );
        summary
    }
}

pub(crate) fn compute_catalog_discovery_checkpoint(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    audit_rows: &[CatalogAuditRow],
) -> CatalogDiscoveryCheckpoint {
    let started = Instant::now();
    let mut lines = vec![
        format!("schema\t{SCHEMA_VERSION}"),
        format!("catalog-build\t{CATALOG_BUILD_VERSION}"),
        format!("profile-set\t{PROFILE_SET_VERSION}"),
        format!(
            "core-launch-manifest\t{}\t{:016x}",
            CORE_LAUNCH_MANIFEST_VERSION,
            core_launch_manifest_fingerprint()
        ),
        format!("roots\t{}", roots.len()),
    ];

    let root_t = Instant::now();
    for (idx, root) in roots.iter().enumerate() {
        append_path_signature(&mut lines, "root", idx, Path::new(root));
    }
    report_checkpoint_timing(
        "roots",
        root_t.elapsed().as_micros() as u64,
        format!("roots={}", roots.len()),
    );

    let core_t = Instant::now();
    append_core_summaries(&mut lines, roots);
    report_checkpoint_timing(
        "cores",
        core_t.elapsed().as_micros() as u64,
        format!("lines={}", lines.len()),
    );

    let game_dir_t = Instant::now();
    append_game_dir_summaries(&mut lines, roots);
    report_checkpoint_timing(
        "game_dirs",
        game_dir_t.elapsed().as_micros() as u64,
        format!("lines={}", lines.len()),
    );

    let audit_t = Instant::now();
    append_audit_summary(&mut lines, audit_rows);
    report_checkpoint_timing(
        "audit",
        audit_t.elapsed().as_micros() as u64,
        format!("rows={}", audit_rows.len()),
    );

    append_named_file_signature(&mut lines, "mame-metadata", mame_sqlite_path);
    append_named_file_signature(&mut lines, "hbmame-metadata", hbmame_sqlite_path);
    report_checkpoint_timing(
        "compute_total",
        started.elapsed().as_micros() as u64,
        format!("lines={}", lines.len()),
    );
    CatalogDiscoveryCheckpoint { lines }
}

pub(crate) fn catalog_trace_detail_enabled() -> bool {
    std::env::var("MISTER_CATALOG_TRACE")
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("detail")
}

pub(crate) fn report_checkpoint_timing(stage: &str, us: u64, detail: impl std::fmt::Display) {
    println!("catalog_checkpoint_tsv\t{stage}\t{us}\t{detail}");
}

pub(crate) fn report_drift_summary(summary: &CatalogDriftSummary) {
    println!(
        "catalog_drift_tsv\tunchanged={}\tadded={}\tremoved={}\tversions={}\troots={}\tcore_roots={}\tcores={}\tgame_dirs={}\taudit={}\tmetadata={}\tdetail={}",
        summary.unchanged,
        summary.added_lines,
        summary.removed_lines,
        summary.changed_versions,
        summary.changed_roots,
        summary.changed_core_roots,
        summary.changed_cores,
        summary.changed_game_dirs,
        summary.changed_audit_rows,
        summary.changed_metadata,
        summary.detail
    );
}

fn append_core_summaries(lines: &mut Vec<String>, roots: &[String]) {
    let search_roots = core_search_roots(roots);
    lines.push(format!("core-search-roots\t{}", search_roots.len()));
    let mut core_count = 0usize;
    for (idx, search_root) in search_roots.iter().enumerate() {
        append_path_signature(lines, "core-search-root", idx, search_root);
        if !search_root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(search_root)
            .follow_links(false)
            .max_depth(3)
            .into_iter()
            .filter_entry(|entry| !should_ignore_path(entry.path()))
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if !entry.file_type().is_file() || !path_ext_eq(path, "rbf") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.eq_ignore_ascii_case("menu") {
                continue;
            }
            let core_id = canonical_core_id(stem);
            let status = if is_known_core(&core_id) {
                "known"
            } else {
                "unknown"
            };
            append_installed_core_signature(lines, core_count, status, &core_id, path);
            core_count += 1;
        }
    }
    lines.push(format!("installed-cores\t{core_count}"));
}

fn append_game_dir_summaries(lines: &mut Vec<String>, roots: &[String]) {
    let game_roots = game_roots(roots);
    lines.push(format!("game-roots\t{}", game_roots.len()));
    let mut dir_count = 0usize;
    for (root_idx, game_root) in game_roots.iter().enumerate() {
        append_path_signature(lines, "game-root", root_idx, game_root);
        let Ok(read_dir) = std::fs::read_dir(game_root) else {
            continue;
        };
        let mut dirs = Vec::new();
        for entry in read_dir.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if should_ignore_game_dir(name) {
                continue;
            }
            dirs.push((name.to_string(), path, metadata));
        }
        dirs.sort_by_key(|entry| entry.0.to_ascii_lowercase());
        for (name, path, metadata) in dirs {
            let status = if launch_profiles::generic_manifest_profile_for_game_dir(&name).is_some()
                || launch_profiles::builtin_profiles().iter().any(|profile| {
                    profile
                        .game_dirs
                        .iter()
                        .any(|dir| dir.eq_ignore_ascii_case(&name))
                }) {
                "known"
            } else {
                "unknown"
            };
            let payloadish = game_dir_has_payloadish_files(&path);
            lines.push(format!(
                "game-dir\t{dir_count}\t{}\t{}\t{}\t{}\t{}\t{}",
                path.display(),
                name,
                status,
                if payloadish { "payloadish" } else { "empty" },
                metadata.len(),
                mtime_nanos(&metadata)
            ));
            dir_count += 1;
        }
    }
    lines.push(format!("game-dirs\t{dir_count}"));
}

fn append_audit_summary(lines: &mut Vec<String>, audit_rows: &[CatalogAuditRow]) {
    let mut counts = BTreeMap::<String, usize>::new();
    for row in audit_rows {
        *counts
            .entry(format!(
                "{}\t{}\t{}",
                row.catalog_status, row.source, row.reason
            ))
            .or_default() += 1;
    }
    lines.push(format!("core-audit-summary\t{}", counts.len()));
    for (idx, (key, count)) in counts.into_iter().enumerate() {
        lines.push(format!("core-audit-row\t{idx}\t{key}\t{count}"));
    }
}

fn append_path_signature(lines: &mut Vec<String>, kind: &str, idx: usize, path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => lines.push(format!(
            "{kind}\t{idx}\t{}\t{}\t{}\t{}",
            path.display(),
            if meta.is_dir() { "dir" } else { "not-dir" },
            meta.len(),
            mtime_nanos(&meta)
        )),
        Err(_) => lines.push(format!("{kind}\t{idx}\t{}\tmissing", path.display())),
    }
}

fn append_installed_core_signature(
    lines: &mut Vec<String>,
    idx: usize,
    status: &str,
    core_id: &str,
    path: &Path,
) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            lines.push(format!(
                "installed-core\t{idx}\t{status}\t{core_id}\t{}\t{}\t{}",
                path.display(),
                meta.len(),
                mtime_nanos(&meta)
            ));
            if catalog_trace_detail_enabled() {
                println!(
                    "catalog_profile_manifest_tsv\tcore_id={core_id}\tstatus={status}\tpath={}\tsize={}\tmtime_nanos={}",
                    path.display(),
                    meta.len(),
                    mtime_nanos(&meta)
                );
            }
        }
        Err(_) => lines.push(format!(
            "installed-core\t{idx}\t{status}\t{core_id}\t{}\tmissing",
            path.display()
        )),
    }
}

fn append_named_file_signature(lines: &mut Vec<String>, name: &str, path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => lines.push(format!(
            "{name}\t{}\t{}\t{}",
            path.display(),
            meta.len(),
            mtime_nanos(&meta)
        )),
        Err(_) => lines.push(format!("{name}\t{}\tmissing", path.display())),
    }
}

fn core_search_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        let root = Path::new(root);
        let candidates = if path_name_eq(root, "games") {
            let base = root.parent().unwrap_or(root);
            vec![
                base.join("_Console"),
                base.join("_Computer"),
                base.join("_Arcade/cores"),
                base.join("_LLAPI"),
            ]
        } else if path_name_eq(root, "_Arcade") {
            vec![root.join("cores")]
        } else if path_name_eq(root, "_Console")
            || path_name_eq(root, "_Computer")
            || path_name_eq(root, "_LLAPI")
        {
            vec![root.to_path_buf()]
        } else {
            vec![
                root.join("_Console"),
                root.join("_Computer"),
                root.join("_Arcade/cores"),
                root.join("_LLAPI"),
            ]
        };
        for candidate in candidates {
            let key = candidate.display().to_string().to_ascii_lowercase();
            if seen.insert(key) {
                out.push(candidate);
            }
        }
    }
    out
}

fn game_roots(roots: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        let path = Path::new(root);
        let games = if path_name_eq(path, "games") {
            path.to_path_buf()
        } else {
            path.join("games")
        };
        let key = games.display().to_string().to_ascii_lowercase();
        if seen.insert(key) {
            out.push(games);
        }
    }
    out
}

fn game_dir_has_payloadish_files(path: &Path) -> bool {
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_depth(2)
        .into_iter()
        .filter_entry(|entry| !should_ignore_path(entry.path()))
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        return true;
    }
    false
}

fn is_known_core(core_id: &str) -> bool {
    launch_profiles::generic_manifest_profile_for_core(core_id).is_some()
        || launch_profiles::builtin_profiles()
            .iter()
            .any(|profile| profile.core_name.eq_ignore_ascii_case(core_id))
}

fn canonical_core_id(stem: &str) -> String {
    let mut core = stem;
    if let Some((prefix, suffix)) = stem.rsplit_once('_') {
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            core = prefix;
        }
    }
    core.to_string()
}

fn should_ignore_game_dir(name: &str) -> bool {
    (name.len() > 1 && name.starts_with('.'))
        || name.eq_ignore_ascii_case("palettes")
        || name.eq_ignore_ascii_case("images")
        || name.eq_ignore_ascii_case("manuals")
        || name.eq_ignore_ascii_case("screenshot")
        || name.eq_ignore_ascii_case("screenshots")
        || name.eq_ignore_ascii_case("screenshot-magik")
        || name.eq_ignore_ascii_case("_organized")
        || name.eq_ignore_ascii_case("boxart")
}

fn path_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

fn path_ext_eq(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

fn mtime_nanos(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn fnv64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(CHECKPOINT_HASH_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn checkpoint_is_stable_for_unchanged_roots() {
        let root = unique_temp_dir("checkpoint-stable");
        std::fs::create_dir_all(root.join("games/NES")).expect("create nes dir");
        std::fs::write(root.join("games/NES/Game.nes"), b"rom").expect("write rom");
        set_mtime_for_test(&root.join("games"), 10, 0);
        let roots = vec![root.display().to_string()];

        let first =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);
        let second =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);

        assert_eq!(first, second);
        assert_eq!(first.fingerprint_hex(), second.fingerprint_hex());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn known_core_addition_changes_checkpoint() {
        let root = unique_temp_dir("checkpoint-known-core");
        let roots = vec![root.display().to_string()];
        let first =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);

        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console");
        std::fs::write(console.join("ColecoVision_20260630.rbf"), b"core").expect("write core");
        let second =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);

        let drift = CatalogDriftSummary::from_checkpoints(Some(&first), &second);
        assert!(!drift.unchanged);
        assert!(drift.changed_cores > 0 || drift.changed_core_roots > 0);
        assert!(second
            .lines()
            .iter()
            .any(|line| line.contains("installed-core") && line.contains("known")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_core_and_game_dir_are_checkpoint_inputs() {
        let root = unique_temp_dir("checkpoint-unknown-core");
        let roots = vec![root.display().to_string()];
        let first =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);

        let console = root.join("_Console");
        let game_dir = root.join("games/ChannelF");
        std::fs::create_dir_all(&console).expect("create console");
        std::fs::create_dir_all(&game_dir).expect("create game dir");
        std::fs::write(console.join("ChannelF_20260630.rbf"), b"core").expect("write core");
        std::fs::write(game_dir.join("Alien.chf"), b"rom").expect("write payload");
        let second =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);

        let drift = CatalogDriftSummary::from_checkpoints(Some(&first), &second);
        assert!(!drift.unchanged);
        assert!(drift.changed_cores > 0 || drift.changed_game_dirs > 0);
        assert!(second
            .lines()
            .iter()
            .any(|line| line.contains("installed-core") && line.contains("unknown")));
        assert!(second
            .lines()
            .iter()
            .any(|line| line.contains("game-dir") && line.contains("payloadish")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ignored_media_dirs_do_not_become_game_dir_lines() {
        let root = unique_temp_dir("checkpoint-ignore-media");
        for dir in [
            "screenshots",
            "ScreenShot",
            "screenshot-magik",
            "BoxArt",
            "_organized",
        ] {
            let path = root.join("games").join(dir);
            std::fs::create_dir_all(&path).expect("create ignored dir");
            std::fs::write(path.join("Fake.nes"), b"media").expect("write fake media payload");
        }
        let roots = vec![root.display().to_string()];

        let checkpoint =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);

        for dir in [
            "screenshots",
            "ScreenShot",
            "screenshot-magik",
            "BoxArt",
            "_organized",
        ] {
            assert!(
                !checkpoint
                    .lines()
                    .iter()
                    .any(|line| line.contains("game-dir") && line.contains(dir)),
                "{dir} should not be a checkpoint game-dir line"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn appledouble_core_sidecars_are_ignored() {
        let root = unique_temp_dir("checkpoint-appledouble-core");
        let console = root.join("_Console");
        std::fs::create_dir_all(&console).expect("create console");
        std::fs::write(console.join("._ColecoVision_20260630.rbf"), b"sidecar")
            .expect("write sidecar");
        let roots = vec![root.display().to_string()];

        let checkpoint =
            compute_catalog_discovery_checkpoint(&roots, &root.join("mame"), &root.join("hbmame"), &[]);

        assert!(!checkpoint
            .lines()
            .iter()
            .any(|line| line.contains("._ColecoVision")));
        let _ = std::fs::remove_dir_all(root);
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[cfg(unix)]
    fn set_mtime_for_test(path: &Path, sec: i64, nsec: i64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        let c_path = CString::new(path.as_os_str().as_bytes()).expect("path CString");
        let times = [
            Timespec {
                tv_sec: sec,
                tv_nsec: nsec,
            },
            Timespec {
                tv_sec: sec,
                tv_nsec: nsec,
            },
        ];
        unsafe extern "C" {
            fn utimensat(
                dirfd: i32,
                pathname: *const i8,
                times: *const Timespec,
                flags: i32,
            ) -> i32;
        }
        let rc = unsafe { utimensat(-100, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed");
    }
}
