// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use serde::Deserialize;
use std::fs;
use std::path::Path;

const PLATFORM_WORKFLOW: &str = "Build MiSTer MagiK Platform";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct ArtifactOrigin {
    id: u64,
    head_branch: String,
    head_sha: String,
    repository_id: u64,
    head_repository_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Artifact {
    id: u64,
    name: String,
    expired: bool,
    created_at: String,
    workflow_run: ArtifactOrigin,
}

#[derive(Deserialize)]
struct ArtifactPage {
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ArtifactPages {
    One(ArtifactPage),
    Many(Vec<ArtifactPage>),
}

#[derive(Deserialize)]
struct RunDetails {
    status: String,
    conclusion: String,
    #[serde(rename = "workflowName")]
    workflow_name: String,
    #[serde(rename = "headBranch")]
    head_branch: String,
    event: String,
    #[serde(rename = "headSha")]
    head_sha: String,
}

pub fn print_candidates(path: &Path, name: &str) -> AgentResult<()> {
    for artifact in candidates(path, name)? {
        println!(
            "{}\t{}\t{}",
            artifact.id, artifact.workflow_run.id, artifact.workflow_run.head_sha
        );
    }
    Ok(())
}

pub fn require_eligible_run(path: &Path, expected_sha: &str) -> AgentResult<()> {
    let payload: RunDetails = read_json(path, "Actions run details")?;
    let eligible = payload.status == "completed"
        && matches!(
            payload.conclusion.as_str(),
            "success" | "failure" | "cancelled"
        )
        && payload.workflow_name == PLATFORM_WORKFLOW
        && payload.head_branch == "main"
        && payload.event == "workflow_dispatch"
        && payload.head_sha == expected_sha;
    if eligible {
        Ok(())
    } else {
        Err(AgentError::Classified {
            code: "platform_run_ineligible",
            detail: "run is not a completed unified main-branch workflow_dispatch for the expected commit".into(),
        })
    }
}

pub fn require_alpha_promotion(
    channel: &str,
    alpha_sha: &str,
    candidate_sha: &str,
) -> AgentResult<()> {
    if channel != "beta" {
        return Ok(());
    }
    if alpha_sha.is_empty() {
        return Err(AgentError::Classified {
            code: "alpha_release_missing",
            detail: "beta publication requires an existing alpha tag".into(),
        });
    }
    if alpha_sha != candidate_sha {
        return Err(AgentError::Classified {
            code: "alpha_commit_mismatch",
            detail: format!(
                "beta requires tested alpha commit {alpha_sha}; candidate is {candidate_sha}"
            ),
        });
    }
    Ok(())
}

fn candidates(path: &Path, name: &str) -> AgentResult<Vec<Artifact>> {
    let pages: ArtifactPages = read_json(path, "Actions artifacts response")?;
    let artifacts = match pages {
        ArtifactPages::One(page) => page.artifacts,
        ArtifactPages::Many(pages) => pages.into_iter().flat_map(|page| page.artifacts).collect(),
    };
    let mut result: Vec<_> = artifacts
        .into_iter()
        .filter(|artifact| {
            artifact.name == name
                && !artifact.expired
                && artifact.workflow_run.head_branch == "main"
                && artifact.workflow_run.repository_id == artifact.workflow_run.head_repository_id
        })
        .collect();
    result.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(result)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, label: &str) -> AgentResult<T> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| AgentError::Classified {
        code: "invalid_ci_metadata",
        detail: format!("invalid {label}: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(payload: &Value) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "agent-cli-artifacts-{}-{nonce}.json",
            std::process::id()
        ));
        fs::write(&path, serde_json::to_vec(payload).unwrap()).unwrap();
        path
    }

    fn artifact(id: u64, created_at: &str) -> Artifact {
        Artifact {
            id,
            name: "wanted".into(),
            expired: false,
            created_at: created_at.into(),
            workflow_run: ArtifactOrigin {
                id: id + 100,
                head_branch: "main".into(),
                head_sha: format!("{id:040x}"),
                repository_id: 1,
                head_repository_id: 1,
            },
        }
    }

    #[test]
    fn candidate_order_is_newest_first() {
        let mut values = [artifact(1, "2026-01-01"), artifact(2, "2026-01-03")];
        values.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        assert_eq!(
            values.iter().map(|value| value.id).collect::<Vec<_>>(),
            [2, 1]
        );
    }

    #[test]
    fn candidate_selection_filters_and_orders_exact_reusable_artifacts() {
        let payload = json!({"artifacts":[
            {"id":1,"name":"wanted","expired":false,"created_at":"2026-01-01","workflow_run":{"id":101,"head_branch":"main","head_sha":"1111111111111111111111111111111111111111","repository_id":1,"head_repository_id":1}},
            {"id":2,"name":"wanted","expired":false,"created_at":"2026-01-03","workflow_run":{"id":102,"head_branch":"main","head_sha":"2222222222222222222222222222222222222222","repository_id":1,"head_repository_id":1}},
            {"id":3,"name":"wanted","expired":true,"created_at":"2026-01-04","workflow_run":{"id":103,"head_branch":"main","head_sha":"3333333333333333333333333333333333333333","repository_id":1,"head_repository_id":1}},
            {"id":4,"name":"other","expired":false,"created_at":"2026-01-05","workflow_run":{"id":104,"head_branch":"main","head_sha":"4444444444444444444444444444444444444444","repository_id":1,"head_repository_id":1}}
        ]});
        let path = fixture(&payload);
        let selected = candidates(&path, "wanted").unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|artifact| artifact.id)
                .collect::<Vec<_>>(),
            [2, 1]
        );
    }

    #[test]
    fn candidate_selection_handles_missing_and_malformed_metadata() {
        let missing = fixture(&json!({"artifacts":[]}));
        assert!(candidates(&missing, "wanted").unwrap().is_empty());
        fs::remove_file(missing).unwrap();

        let malformed = fixture(&json!({"artifacts":[{"id":"not-a-number"}]}));
        assert!(candidates(&malformed, "wanted").is_err());
        fs::remove_file(malformed).unwrap();
    }

    #[test]
    fn beta_requires_the_alpha_commit() {
        assert!(require_alpha_promotion("alpha", "", "a").is_ok());
        assert!(require_alpha_promotion("beta", "", "a").is_err());
        assert!(require_alpha_promotion("beta", "a", "b").is_err());
        assert!(require_alpha_promotion("beta", "a", "a").is_ok());
    }
}
