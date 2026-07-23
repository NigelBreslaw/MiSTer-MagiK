// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("agent-cli-{label}-{}-{nonce}", std::process::id()))
}

fn digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut output, "{byte:02x}").unwrap();
    }
    output
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-cli"))
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
        .env("MISTER_AGENT_CLI_STATE_DIR", root.join("state"))
        .output()
        .unwrap()
}

#[test]
fn platform_candidates_keep_payload_on_stdout_and_progress_on_stderr() {
    let root = temp_root("candidates");
    fs::create_dir_all(&root).unwrap();
    let artifacts = root.join("artifacts.json");
    fs::write(
        &artifacts,
        serde_json::to_vec(&json!({"artifacts":[{
            "id":17,
            "name":"wanted",
            "expired":false,
            "created_at":"2026-07-22T00:00:00Z",
            "workflow_run":{
                "id":29,
                "head_branch":"main",
                "head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "repository_id":1,
                "head_repository_id":1
            }
        }]}))
        .unwrap(),
    )
    .unwrap();

    let output = run(
        &root,
        &[
            "ci",
            "platform-candidates",
            artifacts.to_str().unwrap(),
            "wanted",
        ],
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "17\t29\taaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Selecting reusable platform artifacts")
    );
}

#[test]
fn verify_component_keeps_json_on_stdout_and_progress_on_stderr() {
    let root = temp_root("verify-component");
    let artifact = root.join("kernel");
    fs::create_dir_all(&artifact).unwrap();
    let component_id = "b".repeat(64);
    let head_sha = "c".repeat(40);
    let module = b"kernel-module";
    let provenance = format!(
        "component_input_sha256={component_id}\nmodule_sha256={}\nplatform_contract_sha256={}\n",
        digest(module),
        "d".repeat(64)
    );
    let origin = serde_json::to_vec_pretty(&json!({
        "format":"mister-magik-platform-component-origin-v1",
        "component":"kernel",
        "component_id":component_id.clone(),
        "workflow":"platform-bundle.yml",
        "run_id":"123",
        "head_sha":head_sha.clone(),
        "head_branch":"main"
    }))
    .unwrap();
    fs::write(artifact.join("mister_magik_scanout_slots.ko"), module).unwrap();
    fs::write(artifact.join("provenance.txt"), &provenance).unwrap();
    fs::write(artifact.join("platform-component-origin-v1.json"), &origin).unwrap();
    fs::write(
        artifact.join("SHA256SUMS"),
        format!("{}  mister_magik_scanout_slots.ko\n", digest(module)),
    )
    .unwrap();
    fs::write(
        artifact.join("platform-component-SHA256SUMS"),
        format!(
            "{}  SHA256SUMS\n{}  mister_magik_scanout_slots.ko\n{}  platform-component-origin-v1.json\n{}  provenance.txt\n",
            digest(
                format!("{}  mister_magik_scanout_slots.ko\n", digest(module)).as_bytes()
            ),
            digest(module),
            digest(&origin),
            digest(provenance.as_bytes())
        ),
    )
    .unwrap();

    let output = run(
        &root,
        &[
            "ci",
            "platform-bundle",
            "verify-component",
            "--component",
            "kernel",
            "--artifact",
            artifact.to_str().unwrap(),
            "--component-id",
            &component_id,
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["origin"]["run_id"], "123");
    assert_eq!(payload["origin"]["head_sha"], head_sha);
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Processing platform bundle")
    );
}
