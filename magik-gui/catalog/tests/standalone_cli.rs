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
