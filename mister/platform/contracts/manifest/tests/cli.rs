// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_platform_manifest_contract::{
    Layout, ValidationProfile, parse, qualification_candidate_id, serialize,
};
use std::{fs, process::Command};

#[test]
fn strict_cli_rejects_the_published_dev_as_public_regression() {
    let directory = std::env::temp_dir().join(format!("magik-manifest-cli-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("platform-v3.manifest");
    let public = include_str!("../../generated/platform-v3.public.fixture");
    let dev = include_str!("../../generated/platform-v3.development.fixture");
    let check = |text: &str, layout: &str| {
        fs::write(&path, text).unwrap();
        Command::new(env!("CARGO_BIN_EXE_platform-manifest-check"))
            .args(["--layout", layout, "--manifest"])
            .arg(&path)
            .output()
            .unwrap()
    };
    assert!(check(public, "public").status.success());
    assert!(check(dev, "dev").status.success());
    for (text, layout) in [(dev, "public"), (public, "dev")] {
        let result = check(text, layout);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("platform_path_mismatch"));
    }
    for (component, _) in Layout::Public.paths().components() {
        let mut values = parse(public, Layout::Public, ValidationProfile::AgentStrict)
            .unwrap()
            .into_values();
        values.insert(format!("{component}_path"), "/media/fat/wrong".into());
        values.insert(
            "qualification_candidate_id".into(),
            qualification_candidate_id(&values),
        );
        let result = check(&serialize(&values).unwrap(), "public");
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("platform_path_mismatch"));
    }
    for text in [
        format!("{public}manager_path=/media/fat/duplicate\n"),
        public
            .lines()
            .filter(|line| !line.starts_with("manager_path="))
            .collect::<Vec<_>>()
            .join("\n"),
        public.replace("manager_sha256=", "manager_sha256=INVALID"),
        public.replace("latch_protocol_version=5", "latch_protocol_version=0"),
    ] {
        assert!(!check(&text, "public").status.success());
    }
    assert!(!check(public, "unknown").status.success());
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_platform-manifest-check"))
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    fs::remove_dir_all(directory).unwrap();
}
