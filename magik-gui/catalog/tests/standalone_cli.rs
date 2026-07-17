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
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("files=9"));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["format"],
        "mister-magik-synthetic-catalog-fixture-v1"
    );
    assert_eq!(manifest["files"], 9);
    assert!(fixture
        .join("games/C64/level-00-00/level-01-00/bucket-00000000/Synthetic C64 00000002.d64")
        .is_file());
    std::fs::remove_dir_all(temp).unwrap();
}
