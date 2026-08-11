// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(feature = "builder")]
#[test]
fn standalone_check_emits_json_handshake_and_terminal_failure() {
    use std::process::Command;

    let temp = std::env::temp_dir().join(format!(
        "mister-magik-catalog-builder-cli-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mister-magik-catalog-builder"))
        .arg("check")
        .env("MISTER_CATALOG_BUILDER_LOCK", temp.join("builder.lock"))
        .env("MISTER_LIBRARY_SQLITE", temp.join("missing.sqlite3"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.first().unwrap()["event"], "handshake");
    assert_eq!(events.first().unwrap()["operation"], "check");
    assert_eq!(events.last().unwrap()["event"], "failure");
    assert_eq!(events.last().unwrap()["stage"], "check");
    let _ = std::fs::remove_dir_all(temp);
}

#[cfg(feature = "builder")]
#[test]
fn catalog_lab_creates_a_deterministic_synthetic_library() {
    use std::process::Command;

    let temp = std::env::temp_dir().join(format!(
        "mister-magik-catalog-lab-cli-{}",
        std::process::id()
    ));
    let fixture = temp.join("fixture");
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir(&temp).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_catalog-lab"))
        .args([
            "fixture",
            fixture.to_str().unwrap(),
            "--arcade-games",
            "1",
            "--small-system-games",
            "2",
            "--large-system-games",
            "3",
            "--large-system-depth",
            "2",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("files=9")
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["format"],
        "mister-magik-synthetic-catalog-fixture-v1"
    );
    assert_eq!(manifest["files"], 9);
    assert!(
        fixture
            .join("games/C64/level-00-00/level-01-00/bucket-00000000/Synthetic C64 00000002.d64")
            .is_file()
    );
    std::fs::remove_dir_all(temp).unwrap();
}

#[cfg(feature = "builder")]
#[test]
fn catalog_lab_bootstraps_and_reopens_a_fixture_without_magik() {
    use std::process::Command;

    let temp = std::env::temp_dir().join(format!(
        "mister-magik-catalog-lab-bootstrap-cli-{}",
        std::process::id()
    ));
    let source = temp.join("source");
    let storage = temp.join("catalog");
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir(&temp).unwrap();
    let fixture = Command::new(env!("CARGO_BIN_EXE_catalog-lab"))
        .args([
            "fixture",
            source.to_str().unwrap(),
            "--arcade-games",
            "0",
            "--small-system-games",
            "2",
            "--large-system-games",
            "0",
        ])
        .output()
        .unwrap();
    assert!(fixture.status.success(), "{:?}", fixture);

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_catalog-lab"))
            .args([
                "bootstrap-fixture",
                source.to_str().unwrap(),
                storage.to_str().unwrap(),
                "snes",
            ])
            .output()
            .unwrap()
    };
    let first = run();
    assert!(first.status.success(), "{:?}", first);
    assert!(
        String::from_utf8(first.stdout)
            .unwrap()
            .contains("generation=1\tgames=2\tpublished=1")
    );
    let second = run();
    assert!(second.status.success(), "{:?}", second);
    assert!(
        String::from_utf8(second.stdout)
            .unwrap()
            .contains("generation=1\tgames=2\tpublished=0")
    );
    std::fs::remove_dir_all(temp).unwrap();
}

#[cfg(feature = "builder")]
#[test]
fn production_builder_publishes_v3_without_creating_v2_artifacts() {
    use std::process::Command;

    let temp = std::env::temp_dir().join(format!(
        "mister-magik-v3-only-builder-{}",
        std::process::id()
    ));
    let source = temp.join("source");
    let storage = temp.join("catalog-v3");
    let legacy = temp.join("library.sqlite3");
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir(&temp).unwrap();
    let fixture = Command::new(env!("CARGO_BIN_EXE_catalog-lab"))
        .args([
            "fixture",
            source.to_str().unwrap(),
            "--arcade-games",
            "1",
            "--small-system-games",
            "2",
            "--large-system-games",
            "0",
        ])
        .output()
        .unwrap();
    assert!(fixture.status.success(), "{:?}", fixture);

    let output = Command::new(env!("CARGO_BIN_EXE_mister-magik-catalog-builder"))
        .arg("build")
        .env("MISTER_CATALOG_BUILDER_LOCK", temp.join("builder.lock"))
        .env("MISTER_CATALOG_READY_SNAPSHOT", temp.join("ready.nav.lz4b"))
        .env("MISTER_LIBRARY_ROOTS", source.display().to_string())
        .env("MISTER_SHARDED_CATALOG_DIR", &storage)
        .env("MISTER_LIBRARY_SQLITE", &legacy)
        .env("MISTER_MAME_SQLITE", temp.join("missing-mame.sqlite3"))
        .env("MISTER_HBMAME_SQLITE", temp.join("missing-hbmame.sqlite3"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(storage.join("state/catalog-state.sqlite3").is_file());
    assert!(storage.join("state/scanner-cache.sqlite3").is_file());
    let report = mister_magik_catalog::catalog_acceptance::inspect_catalog(&storage).unwrap();
    assert!(report.contains("catalog_v3_summary_tsv\tvalid=1\tschema=1"));
    assert!(report.contains("\tsystems=2\ttotal_games=3\tnavpack_systems=2\tnavpack_bytes="));
    assert!(report.contains("\tarcade_resident_games=1"));
    assert_eq!(report.matches("catalog_v3_system_tsv").count(), 2);
    assert!(
        report
            .lines()
            .filter(|line| line.starts_with("catalog_v3_system_tsv"))
            .all(|line| line.contains("\tpreview_keys=") && line.contains("\tavailable_previews="))
    );
    assert!(!legacy.exists());
    assert!(!temp.join("library.summary.json").exists());
    assert!(!temp.join("library.nav.lz4b").exists());
    std::fs::remove_dir_all(temp).unwrap();
}
