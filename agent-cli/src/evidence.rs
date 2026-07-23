// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::components::DeploymentImpact;
use crate::model::{Intent, Outcome};
use crate::progress::ProgressEvent;
use crate::request::RawRequest;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS requests (
    id TEXT PRIMARY KEY,
    started_ms INTEGER NOT NULL,
    completed_ms INTEGER,
    args_json TEXT NOT NULL,
    parse_status TEXT NOT NULL DEFAULT 'captured',
    intent_json TEXT,
    plan_json TEXT,
    rejection_reason TEXT,
    outcome TEXT,
    bootstrap_ms INTEGER,
    execution_started_ms INTEGER,
    execution_ms INTEGER
);
CREATE TABLE IF NOT EXISTS commands (
    id INTEGER PRIMARY KEY,
    request_id TEXT NOT NULL REFERENCES requests(id),
    operation_id TEXT NOT NULL,
    program TEXT NOT NULL,
    args_json TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    completed_ms INTEGER,
    duration_ms INTEGER,
    exit_code INTEGER,
    status TEXT NOT NULL,
    log_path TEXT
);
CREATE TABLE IF NOT EXISTS events (
    request_id TEXT NOT NULL REFERENCES requests(id),
    sequence INTEGER NOT NULL,
    elapsed_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    phase TEXT NOT NULL,
    message TEXT NOT NULL,
    percent INTEGER,
    PRIMARY KEY (request_id, sequence)
);
CREATE TABLE IF NOT EXISTS tasks (
    task_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    worktree TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    baseline_json TEXT NOT NULL,
    closed_ms INTEGER,
    commit_sha TEXT
);
CREATE TABLE IF NOT EXISTS operation_cache (
    task_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    completed_ms INTEGER NOT NULL,
    PRIMARY KEY (task_id, operation_id, fingerprint)
);
CREATE TABLE IF NOT EXISTS task_claims (
    task_id TEXT NOT NULL REFERENCES tasks(task_id),
    path TEXT NOT NULL,
    claimed_ms INTEGER NOT NULL,
    PRIMARY KEY (task_id, path)
);
CREATE TABLE IF NOT EXISTS commit_attempts (
    request_id TEXT PRIMARY KEY REFERENCES requests(id),
    task_id TEXT NOT NULL,
    message TEXT NOT NULL,
    paths_json TEXT NOT NULL,
    status TEXT NOT NULL,
    commit_sha TEXT,
    subject TEXT,
    rolled_back INTEGER NOT NULL DEFAULT 0,
    detail TEXT
);
CREATE TABLE IF NOT EXISTS deliveries (
    task_id TEXT PRIMARY KEY REFERENCES tasks(task_id),
    worktree TEXT NOT NULL,
    source_tree TEXT NOT NULL,
    commit_sha TEXT,
    impact TEXT NOT NULL,
    state TEXT NOT NULL,
    requirement_id TEXT,
    detail TEXT,
    updated_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS delivery_attestations (
    task_id TEXT NOT NULL REFERENCES deliveries(task_id),
    requirement_id TEXT NOT NULL,
    workflow TEXT NOT NULL,
    branch TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (task_id, requirement_id)
);
PRAGMA user_version = 8;
"#;

fn baseline_head(value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get("head")?
        .as_str()
        .map(str::to_owned)
}

#[derive(Debug)]
pub struct Evidence {
    connection: Connection,
    root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DatabaseStatus {
    pub path: PathBuf,
    pub requests: i64,
    pub commands: i64,
    pub events: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub started_ms: i64,
    pub completed_ms: Option<i64>,
    pub parse_status: String,
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RunDetail {
    pub id: String,
    pub args: serde_json::Value,
    pub parse_status: String,
    pub intent: Option<serde_json::Value>,
    pub rejection_reason: Option<String>,
    pub outcome: Option<String>,
    pub commands: Vec<CommandDetail>,
    pub events: Vec<EventDetail>,
    pub commit_attempt: Option<CommitAttemptDetail>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CommandDetail {
    pub operation_id: String,
    pub program: String,
    pub args: serde_json::Value,
    pub duration_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub status: String,
    pub log_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventDetail {
    pub sequence: i64,
    pub elapsed_ms: i64,
    pub kind: String,
    pub phase: String,
    pub message: String,
    pub percent: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeliveryRecord {
    pub task_id: String,
    pub worktree: PathBuf,
    pub source_tree: String,
    pub commit_sha: Option<String>,
    pub impact: DeploymentImpact,
    pub state: DeliveryState,
    pub requirement_id: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Committed,
    ExternalPending,
    ExternalVerified,
    RecoveryRequired,
    Failed,
    Complete,
}

impl DeliveryState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::ExternalPending => "external_pending",
            Self::ExternalVerified => "external_verified",
            Self::RecoveryRequired => "recovery_required",
            Self::Failed => "failed",
            Self::Complete => "complete",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "committed" => Ok(Self::Committed),
            "external_pending" => Ok(Self::ExternalPending),
            "external_verified" => Ok(Self::ExternalVerified),
            "recovery_required" => Ok(Self::RecoveryRequired),
            "failed" => Ok(Self::Failed),
            "complete" => Ok(Self::Complete),
            _ => Err(format!("invalid persisted delivery state: {value}")),
        }
    }

    #[must_use]
    pub const fn can_resume(self) -> bool {
        matches!(
            self,
            Self::ExternalPending | Self::RecoveryRequired | Self::Failed | Self::Complete
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CommitAttemptDetail {
    pub task_id: String,
    pub message: String,
    pub paths: serde_json::Value,
    pub status: String,
    pub commit_sha: Option<String>,
    pub subject: Option<String>,
    pub rolled_back: bool,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommittedTask {
    pub task_id: String,
    pub commit_sha: String,
    pub paths: Vec<PathBuf>,
}

impl Evidence {
    pub fn open_for_repository(repository: &Path) -> Result<Self, String> {
        if let Some(root) = std::env::var_os("MISTER_AGENT_CLI_STATE_DIR") {
            return Self::open_at(Path::new(&root));
        }
        let primary = primary_worktree(repository)?;
        let root = primary.join(".agent-cli");
        migrate_common_dir_database(repository, &root)?;
        Self::open_at(&root)
    }

    pub fn open_at(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|error| {
            format!("cannot create audit directory {}: {error}", root.display())
        })?;
        fs::create_dir_all(root.join("logs"))
            .map_err(|error| format!("cannot create audit log directory: {error}"))?;
        let connection = Connection::open(root.join("agent.sqlite3"))
            .map_err(|error| format!("cannot open audit database: {error}"))?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .and_then(|()| connection.pragma_update(None, "foreign_keys", true))
            .and_then(|()| connection.execute_batch(SCHEMA))
            .map_err(|error| format!("cannot migrate audit database: {error}"))?;
        ensure_column(&connection, "tasks", "closed_ms", "INTEGER")?;
        ensure_column(&connection, "tasks", "commit_sha", "TEXT")?;
        ensure_column(&connection, "tasks", "session_id", "TEXT")?;
        ensure_column(
            &connection,
            "tasks",
            "generation",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        connection
            .execute(
                "UPDATE tasks SET session_id=task_id WHERE session_id IS NULL",
                [],
            )
            .map_err(|error| format!("cannot migrate tasks.session_id: {error}"))?;
        connection
            .execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS tasks_session_generation ON tasks(session_id, generation)",
                [],
            )
            .map_err(|error| format!("cannot index task lifecycles: {error}"))?;
        ensure_column(&connection, "commit_attempts", "subject", "TEXT")?;
        ensure_column(
            &connection,
            "commit_attempts",
            "rolled_back",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(&connection, "requests", "bootstrap_ms", "INTEGER")?;
        ensure_column(&connection, "requests", "execution_started_ms", "INTEGER")?;
        ensure_column(&connection, "requests", "execution_ms", "INTEGER")?;
        connection
            .execute(
                "DELETE FROM operation_cache WHERE task_id NOT IN (SELECT task_id FROM tasks WHERE closed_ms IS NULL)",
                [],
            )
            .map_err(|error| format!("cannot prune operation cache: {error}"))?;
        Ok(Self {
            connection,
            root: root.to_path_buf(),
        })
    }

    fn legacy_common_dir(repository: &Path) -> Result<PathBuf, String> {
        let output = Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(repository)
            .output()
            .map_err(|error| format!("cannot locate Git common directory: {error}"))?;
        if !output.status.success() {
            return Err("agent-cli must run inside a Git repository".into());
        }
        let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let common = if common.is_absolute() {
            common
        } else {
            repository.join(common)
        };
        Ok(common.join("agent-cli"))
    }

    pub fn begin_request(&self, request: &RawRequest) -> Result<(), String> {
        let args = serde_json::to_string(&request.args).map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "INSERT INTO requests (id, started_ms, args_json) VALUES (?1, ?2, ?3)",
                params![request.id, request.started_ms, args],
            )
            .map_err(|error| format!("cannot record request: {error}"))?;
        Ok(())
    }

    pub fn record_intent(&self, request_id: &str, intent: &Intent) -> Result<(), String> {
        let intent = serde_json::to_string(intent).map_err(|error| error.to_string())?;
        let now = now_ms();
        self.connection
            .execute(
                "UPDATE requests SET parse_status = 'parsed', intent_json = ?2, bootstrap_ms=?3-started_ms, execution_started_ms=?3 WHERE id = ?1",
                params![request_id, intent, now],
            )
            .map_err(|error| format!("cannot record parsed intent: {error}"))?;
        Ok(())
    }

    pub fn reject_parse(&self, request_id: &str, reason: &str) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE requests SET completed_ms = ?2, parse_status = 'failed', rejection_reason = ?3, outcome = 'rejected' WHERE id = ?1",
                params![request_id, now_ms(), reason],
            )
            .map_err(|error| format!("cannot record parse rejection: {error}"))?;
        Ok(())
    }

    pub fn finish(&self, request_id: &str, outcome: Outcome) -> Result<(), String> {
        let outcome = serde_json::to_value(outcome)
            .map_err(|error| error.to_string())?
            .as_str()
            .unwrap_or("failed")
            .to_owned();
        self.connection
            .execute(
                "UPDATE requests SET completed_ms = ?2, outcome = ?3, execution_ms=CASE WHEN execution_started_ms IS NULL THEN NULL ELSE ?2-execution_started_ms END WHERE id = ?1",
                params![request_id, now_ms(), outcome],
            )
            .map_err(|error| format!("cannot record request outcome: {error}"))?;
        Ok(())
    }

    pub fn record_event(&self, event: &ProgressEvent) -> Result<(), String> {
        self.record_events(std::slice::from_ref(event))
    }

    pub fn record_events(&self, events: &[ProgressEvent]) -> Result<(), String> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| format!("cannot begin progress transaction: {error}"))?;
        for event in events {
            transaction
                .execute(
                "INSERT INTO events (request_id, sequence, elapsed_ms, kind, phase, message, percent) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![event.run, i64::from(event.seq), i64::try_from(event.elapsed_ms).unwrap_or(i64::MAX), event.kind.as_str(), event.phase, event.message, event.percent.map(i64::from)],
            )
            .map_err(|error| format!("cannot record progress event: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("cannot commit progress events: {error}"))?;
        Ok(())
    }

    pub fn record_plan<T: Serialize>(&self, request_id: &str, plan: &T) -> Result<(), String> {
        let plan = serde_json::to_string(plan).map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "UPDATE requests SET plan_json = ?2 WHERE id = ?1",
                params![request_id, plan],
            )
            .map_err(|error| format!("cannot record plan: {error}"))?;
        Ok(())
    }

    pub fn save_task_baseline<T: Serialize>(
        &self,
        session_id: &str,
        worktree: &Path,
        baseline: &T,
        replace: bool,
    ) -> Result<String, String> {
        let baseline = serde_json::to_string(baseline).map_err(|error| error.to_string())?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        let active: Option<String> = transaction
            .query_row(
                "SELECT task_id FROM tasks WHERE session_id=?1 AND worktree=?2 AND closed_ms IS NULL",
                params![session_id, worktree.display().to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if active.is_some() && !replace {
            return Err(format!(
                "task lifecycle already active for {session_id}; finish it before beginning another"
            ));
        }
        if active.is_some() {
            transaction
                .execute(
                    "UPDATE tasks SET closed_ms=?2 WHERE session_id=?1 AND worktree=?3 AND closed_ms IS NULL",
                    params![session_id, now_ms(), worktree.display().to_string()],
                )
                .map_err(|error| error.to_string())?;
        }
        if session_id.starts_with("task-") {
            transaction
                .execute(
                    "UPDATE tasks SET closed_ms=?2 WHERE worktree=?1 AND session_id LIKE 'task-%' AND closed_ms IS NULL",
                    params![worktree.display().to_string(), now_ms()],
                )
                .map_err(|error| error.to_string())?;
        }
        let generation: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(generation), 0) + 1 FROM tasks WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let task_id = if generation == 1 {
            session_id.to_owned()
        } else {
            format!("{session_id}::g{generation}")
        };
        transaction
            .execute(
                "INSERT INTO tasks (task_id, session_id, generation, worktree, created_ms, baseline_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![task_id, session_id, generation, worktree.display().to_string(), now_ms(), baseline],
            )
            .map_err(|error| {
                if error.to_string().contains("UNIQUE constraint failed") {
                    format!("cannot allocate a new task lifecycle for {session_id}")
                } else {
                    format!("cannot save task baseline: {error}")
                }
            })?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(task_id)
    }

    pub fn active_task_id_for_session(
        &self,
        worktree: &Path,
        session_id: &str,
    ) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT task_id FROM tasks WHERE worktree=?1 AND session_id=?2 AND closed_ms IS NULL ORDER BY generation DESC LIMIT 1",
                params![worktree.display().to_string(), session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn load_task_baseline<T: serde::de::DeserializeOwned>(
        &self,
        task_id: &str,
    ) -> Result<Option<(PathBuf, T)>, String> {
        self.connection
            .query_row(
                "SELECT worktree, baseline_json FROM tasks WHERE task_id = ?1",
                [task_id],
                |row| {
                    let worktree: String = row.get(0)?;
                    let baseline: String = row.get(1)?;
                    Ok((PathBuf::from(worktree), baseline))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?
            .map(|(worktree, baseline)| {
                serde_json::from_str(&baseline)
                    .map(|value| (worktree, value))
                    .map_err(|error| format!("cannot decode task baseline: {error}"))
            })
            .transpose()
    }

    pub fn update_task_baseline<T: Serialize>(
        &self,
        task_id: &str,
        worktree: &Path,
        baseline: &T,
    ) -> Result<(), String> {
        let baseline = serde_json::to_string(baseline).map_err(|error| error.to_string())?;
        let changed = self
            .connection
            .execute(
                "UPDATE tasks SET baseline_json=?3 WHERE task_id=?1 AND worktree=?2 AND closed_ms IS NULL",
                params![task_id, worktree.display().to_string(), baseline],
            )
            .map_err(|error| format!("cannot update task baseline: {error}"))?;
        if changed == 1 {
            Ok(())
        } else {
            Err(format!(
                "task_baseline_missing: no active task baseline exists for {task_id}"
            ))
        }
    }

    pub fn active_task_ids(&self, worktree: &Path, except: &str) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT task_id FROM tasks WHERE worktree=?1 AND closed_ms IS NULL AND task_id<>?2 ORDER BY created_ms",
            )
            .map_err(|error| error.to_string())?;
        let task_ids = statement
            .query_map(params![worktree.display().to_string(), except], |row| {
                row.get(0)
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(task_ids)
    }

    pub fn active_manual_task_id(&self, worktree: &Path) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT session_id FROM tasks WHERE worktree=?1 AND session_id LIKE 'task-%' AND closed_ms IS NULL ORDER BY created_ms DESC LIMIT 1",
                [worktree.display().to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn close_task(&self, task_id: &str, commit_sha: &str) -> Result<(), String> {
        let changed = self
            .connection
            .execute(
                "UPDATE tasks SET closed_ms=?2, commit_sha=?3 WHERE task_id=?1 AND closed_ms IS NULL",
                params![task_id, now_ms(), commit_sha],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            self.connection
                .execute("DELETE FROM operation_cache WHERE task_id=?1", [task_id])
                .map_err(|error| error.to_string())?;
            Ok(())
        } else {
            Err(format!(
                "task_baseline_missing: no active task baseline exists for {task_id}"
            ))
        }
    }

    pub fn supersede_task(&self, worktree: &Path, task_id: &str) -> Result<(), String> {
        let changed = self
            .connection
            .execute(
                "UPDATE tasks SET closed_ms=?3 WHERE task_id=?1 AND worktree=?2 AND closed_ms IS NULL",
                params![task_id, worktree.display().to_string(), now_ms()],
            )
            .map_err(|error| error.to_string())?;
        if changed == 1 {
            self.connection
                .execute("DELETE FROM operation_cache WHERE task_id=?1", [task_id])
                .map_err(|error| error.to_string())?;
            Ok(())
        } else {
            Err(format!(
                "task_supersede_missing: no active task lifecycle exists for {task_id}"
            ))
        }
    }

    pub fn latest_committed_task(
        &self,
        worktree: &Path,
    ) -> Result<Option<(String, String)>, String> {
        self.connection
            .query_row(
                "SELECT task_id, commit_sha FROM tasks WHERE worktree=?1 AND closed_ms IS NOT NULL AND commit_sha IS NOT NULL ORDER BY closed_ms DESC LIMIT 1",
                [worktree.display().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn latest_committed_task_for_session(
        &self,
        worktree: &Path,
        session_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.connection
            .query_row(
                "SELECT task_id, commit_sha FROM tasks WHERE worktree=?1 AND session_id=?2 AND closed_ms IS NOT NULL AND commit_sha IS NOT NULL ORDER BY closed_ms DESC LIMIT 1",
                params![worktree.display().to_string(), session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn latest_committed_scope(&self, worktree: &Path) -> Result<Option<CommittedTask>, String> {
        let Some((task_id, commit_sha)) = self.latest_committed_task(worktree)? else {
            return Ok(None);
        };
        let paths: Option<String> = self
            .connection
            .query_row(
                "SELECT paths_json FROM commit_attempts WHERE task_id=?1 AND commit_sha=?2 AND status='committed' ORDER BY rowid DESC LIMIT 1",
                params![task_id, commit_sha],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let paths = paths.ok_or_else(|| {
            format!(
                "commit_evidence_corrupt: task {task_id} commit {commit_sha} has no successful commit attempt"
            )
        })?;
        let paths: Vec<PathBuf> = serde_json::from_str(&paths).map_err(|error| {
            format!(
                "commit_evidence_corrupt: task {task_id} commit {commit_sha} has invalid committed paths: {error}"
            )
        })?;
        if paths.is_empty() {
            return Err(format!(
                "commit_evidence_corrupt: task {task_id} commit {commit_sha} has no committed paths"
            ));
        }
        Ok(Some(CommittedTask {
            task_id,
            commit_sha,
            paths,
        }))
    }

    pub fn claim_task_paths(&self, task_id: &str, paths: &[PathBuf]) -> Result<(), String> {
        let (worktree, current_baseline): (String, String) = self
            .connection
            .query_row(
                "SELECT worktree, baseline_json FROM tasks WHERE task_id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| format!("cannot load active task identity: {error}"))?;
        let current_head = baseline_head(&current_baseline);
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        for path in paths {
            let path = path.display().to_string();
            let conflict: Option<(String, String)> = transaction
                .query_row(
                    "SELECT c.task_id, t.baseline_json FROM task_claims c JOIN tasks t ON t.task_id=c.task_id WHERE c.path=?1 AND c.task_id<>?2 AND t.closed_ms IS NULL LIMIT 1",
                    params![path, task_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if let Some((other, other_baseline)) = conflict {
                let other_head = baseline_head(&other_baseline);
                let superseded = current_head
                    .as_deref()
                    .zip(other_head.as_deref())
                    .is_some_and(|(current, other)| {
                        current != other
                            && Command::new("git")
                                .args(["merge-base", "--is-ancestor", other, current])
                                .current_dir(&worktree)
                                .status()
                                .map(|status| status.success())
                                .unwrap_or(false)
                    });
                if superseded {
                    transaction
                        .execute(
                            "UPDATE tasks SET closed_ms=?2 WHERE task_id=?1 AND closed_ms IS NULL",
                            params![other, now_ms()],
                        )
                        .map_err(|error| error.to_string())?;
                } else {
                    return Err(format!(
                        "commit_scope_ambiguous: path {path} is already claimed by active task {other}"
                    ));
                }
            }
            transaction
                .execute(
                    "INSERT OR REPLACE INTO task_claims (task_id, path, claimed_ms) VALUES (?1, ?2, ?3)",
                    params![task_id, path, now_ms()],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn task_claims(&self, task_id: &str) -> Result<Vec<PathBuf>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT path FROM task_claims WHERE task_id=?1 ORDER BY path")
            .map_err(|error| error.to_string())?;
        let paths = statement
            .query_map([task_id], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .map(|value| value.map(PathBuf::from))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(paths)
    }

    pub fn save_delivery(&self, delivery: &DeliveryRecord) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO deliveries (task_id, worktree, source_tree, commit_sha, impact, state, requirement_id, detail, updated_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(task_id) DO UPDATE SET source_tree=excluded.source_tree, commit_sha=excluded.commit_sha, impact=excluded.impact, state=excluded.state, requirement_id=excluded.requirement_id, detail=excluded.detail, updated_ms=excluded.updated_ms",
                params![delivery.task_id, delivery.worktree.display().to_string(), delivery.source_tree, delivery.commit_sha, delivery.impact.as_str(), delivery.state.as_str(), delivery.requirement_id, delivery.detail, now_ms()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn delivery(&self, task_id: &str) -> Result<Option<DeliveryRecord>, String> {
        let record = self.connection
            .query_row(
                "SELECT task_id, worktree, source_tree, commit_sha, impact, state, requirement_id, detail FROM deliveries WHERE task_id=?1",
                [task_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        PathBuf::from(row.get::<_, String>(1)?),
                        row.get(2)?,
                        row.get(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        record
            .map(
                |(
                    task_id,
                    worktree,
                    source_tree,
                    commit_sha,
                    impact,
                    state,
                    requirement_id,
                    detail,
                )| {
                    Ok(DeliveryRecord {
                        task_id,
                        worktree,
                        source_tree,
                        commit_sha,
                        impact: DeploymentImpact::parse(&impact)?,
                        state: DeliveryState::parse(&state)?,
                        requirement_id,
                        detail,
                    })
                },
            )
            .transpose()
    }

    pub fn attest_delivery(
        &self,
        task_id: &str,
        requirement_id: &str,
        workflow: &str,
        branch: &str,
        commit_sha: &str,
        evidence_json: &str,
    ) -> Result<(), String> {
        if workflow != "platform-bundle.yml" || branch != "main" {
            return Err("external_attestation_rejected: workflow and branch must be platform-bundle.yml on main".into());
        }
        let delivery = self
            .delivery(task_id)?
            .ok_or("external_attestation_rejected: delivery does not exist")?;
        if delivery.commit_sha.as_deref() != Some(commit_sha) {
            return Err("external_attestation_rejected: commit SHA does not match delivery".into());
        }
        self.connection
            .execute(
                "INSERT OR REPLACE INTO delivery_attestations (task_id, requirement_id, workflow, branch, commit_sha, evidence_json, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![task_id, requirement_id, workflow, branch, commit_sha, evidence_json, now_ms()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn begin_commit_attempt(
        &self,
        request_id: &str,
        task_id: &str,
        message: &str,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO commit_attempts (request_id, task_id, message, paths_json, status) VALUES (?1, ?2, ?3, '[]', 'started')",
                params![request_id, task_id, message],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn update_commit_attempt(
        &self,
        request_id: &str,
        paths: &[PathBuf],
        status: &str,
        commit_sha: Option<&str>,
        subject: Option<&str>,
        detail: Option<&str>,
    ) -> Result<(), String> {
        let paths = serde_json::to_string(paths).map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "UPDATE commit_attempts SET paths_json=?2, status=?3, commit_sha=?4, subject=?5, detail=?6 WHERE request_id=?1",
                params![request_id, paths, status, commit_sha, subject, detail],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn record_commit_rollback(&self, request_id: &str, detail: &str) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE commit_attempts SET status='rolled_back', rolled_back=1, detail=?2 WHERE request_id=?1",
                params![request_id, detail],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn has_cached_operation(
        &self,
        task_id: &str,
        operation_id: &str,
        fingerprint: &str,
    ) -> Result<bool, String> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM operation_cache WHERE task_id=?1 AND operation_id=?2 AND fingerprint=?3)",
                params![task_id, operation_id, fingerprint],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
    }

    pub fn cache_operation(
        &self,
        task_id: &str,
        operation_id: &str,
        fingerprint: &str,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT OR REPLACE INTO operation_cache (task_id, operation_id, fingerprint, completed_ms) VALUES (?1, ?2, ?3, ?4)",
                params![task_id, operation_id, fingerprint, now_ms()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn request_args(&self, request_id: &str) -> Result<Vec<String>, String> {
        let args: String = self
            .connection
            .query_row(
                "SELECT args_json FROM requests WHERE id = ?1",
                [request_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&args).map_err(|error| error.to_string())
    }

    pub fn begin_command(
        &self,
        request_id: &str,
        operation_id: &str,
        program: &str,
        args: &[String],
        log_path: Option<&Path>,
    ) -> Result<i64, String> {
        let args = serde_json::to_string(args).map_err(|error| error.to_string())?;
        self.connection.execute("INSERT INTO commands (request_id, operation_id, program, args_json, started_ms, status, log_path) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6)", params![request_id, operation_id, program, args, now_ms(), log_path.map(|path| path.display().to_string())]).map_err(|error| format!("cannot record command: {error}"))?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn finish_command(
        &self,
        command_id: i64,
        started_ms: i64,
        exit_code: i32,
    ) -> Result<(), String> {
        let completed = now_ms();
        let status = if exit_code == 0 { "passed" } else { "failed" };
        self.connection.execute("UPDATE commands SET completed_ms = ?2, duration_ms = ?3, exit_code = ?4, status = ?5 WHERE id = ?1", params![command_id, completed, completed.saturating_sub(started_ms), exit_code, status]).map_err(|error| format!("cannot record command outcome: {error}"))?;
        Ok(())
    }

    pub fn record_reused_command(
        &self,
        request_id: &str,
        operation_id: &str,
        program: &str,
        args: &[String],
    ) -> Result<(), String> {
        let args = serde_json::to_string(args).map_err(|error| error.to_string())?;
        self.connection.execute(
            "INSERT INTO commands (request_id, operation_id, program, args_json, started_ms, completed_ms, duration_ms, exit_code, status) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, 'reused')",
            params![request_id, operation_id, program, args, now_ms()],
        ).map_err(|error| error.to_string())?;
        Ok(())
    }

    #[must_use]
    pub fn log_path(&self, request_id: &str, operation_id: &str) -> PathBuf {
        let safe = operation_id.replace(['/', '.'], "-");
        self.root
            .join("logs")
            .join(format!("{request_id}-{safe}.log"))
    }

    pub fn status(&self) -> Result<DatabaseStatus, String> {
        Ok(DatabaseStatus {
            path: self.root.join("agent.sqlite3"),
            requests: self.count("requests")?,
            commands: self.count("commands")?,
            events: self.count("events")?,
        })
    }

    pub fn recent_runs(&self, failed: bool, limit: usize) -> Result<Vec<RunSummary>, String> {
        let mut statement = self
            .connection
            .prepare(if failed {
                "SELECT id, started_ms, completed_ms, parse_status, outcome FROM requests WHERE outcome IN ('failed', 'rejected') ORDER BY started_ms DESC LIMIT ?1"
            } else {
                "SELECT id, started_ms, completed_ms, parse_status, outcome FROM requests ORDER BY started_ms DESC LIMIT ?1"
            })
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
                Ok(RunSummary {
                    id: row.get(0)?,
                    started_ms: row.get(1)?,
                    completed_ms: row.get(2)?,
                    parse_status: row.get(3)?,
                    outcome: row.get(4)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn run_detail(&self, id: &str) -> Result<Option<RunDetail>, String> {
        let mut detail = self
            .connection
            .query_row(
                "SELECT id, args_json, parse_status, intent_json, rejection_reason, outcome FROM requests WHERE id = ?1",
                [id],
                |row| {
                    let args: String = row.get(1)?;
                    let intent: Option<String> = row.get(3)?;
                    Ok(RunDetail {
                        id: row.get(0)?,
                        args: serde_json::from_str(&args).unwrap_or_default(),
                        parse_status: row.get(2)?,
                        intent: intent.and_then(|value| serde_json::from_str(&value).ok()),
                        rejection_reason: row.get(4)?,
                        outcome: row.get(5)?,
                        commands: Vec::new(),
                        events: Vec::new(),
                        commit_attempt: None,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some(value) = detail.as_mut() {
            let mut commands = self
                .connection
                .prepare("SELECT operation_id, program, args_json, duration_ms, exit_code, status, log_path FROM commands WHERE request_id = ?1 ORDER BY id")
                .map_err(|error| error.to_string())?;
            value.commands = commands
                .query_map([id], |row| {
                    let args: String = row.get(2)?;
                    Ok(CommandDetail {
                        operation_id: row.get(0)?,
                        program: row.get(1)?,
                        args: serde_json::from_str(&args).unwrap_or_default(),
                        duration_ms: row.get(3)?,
                        exit_code: row.get(4)?,
                        status: row.get(5)?,
                        log_path: row.get(6)?,
                    })
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            let mut events = self
                .connection
                .prepare("SELECT sequence, elapsed_ms, kind, phase, message, percent FROM events WHERE request_id = ?1 ORDER BY sequence")
                .map_err(|error| error.to_string())?;
            value.events = events
                .query_map([id], |row| {
                    Ok(EventDetail {
                        sequence: row.get(0)?,
                        elapsed_ms: row.get(1)?,
                        kind: row.get(2)?,
                        phase: row.get(3)?,
                        message: row.get(4)?,
                        percent: row.get(5)?,
                    })
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            value.commit_attempt = self
                .connection
                .query_row(
                    "SELECT task_id, message, paths_json, status, commit_sha, subject, rolled_back, detail FROM commit_attempts WHERE request_id=?1",
                    [id],
                    |row| {
                        let paths: String = row.get(2)?;
                        Ok(CommitAttemptDetail {
                            task_id: row.get(0)?,
                            message: row.get(1)?,
                            paths: serde_json::from_str(&paths).unwrap_or_default(),
                            status: row.get(3)?,
                            commit_sha: row.get(4)?,
                            subject: row.get(5)?,
                            rolled_back: row.get::<_, i64>(6)? != 0,
                            detail: row.get(7)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| error.to_string())?;
        }
        Ok(detail)
    }

    pub fn prune_logs(&self) -> Result<usize, String> {
        let mut removed = 0;
        let entries = fs::read_dir(self.root.join("logs")).map_err(|error| error.to_string())?;
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_file() {
                fs::remove_file(path).map_err(|error| error.to_string())?;
                removed += 1;
            }
        }
        self.connection
            .execute("UPDATE commands SET log_path = NULL", [])
            .map_err(|error| error.to_string())?;
        Ok(removed)
    }

    fn count(&self, table: &str) -> Result<i64, String> {
        self.connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| error.to_string())
    }
}

fn primary_worktree(repository: &Path) -> Result<PathBuf, String> {
    if let Ok(worktree) = primary_worktree_from_metadata(repository) {
        return Ok(worktree);
    }
    primary_worktree_from_git(repository)
}

fn primary_worktree_from_metadata(repository: &Path) -> Result<PathBuf, String> {
    let worktree = repository
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .ok_or_else(|| "cannot locate Git metadata".to_owned())?;
    let marker = worktree.join(".git");
    if marker.is_dir() {
        return fs::canonicalize(worktree)
            .map_err(|error| format!("cannot resolve primary worktree: {error}"));
    }

    let marker_text = fs::read_to_string(&marker)
        .map_err(|error| format!("cannot read {}: {error}", marker.display()))?;
    let git_dir = marker_text
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{} is not a Git directory pointer", marker.display()))?;
    let git_dir = resolve_metadata_path(worktree, &git_dir);
    let common_text = fs::read_to_string(git_dir.join("commondir"))
        .map_err(|error| format!("cannot read linked-worktree common directory: {error}"))?;
    let common_dir = fs::canonicalize(resolve_metadata_path(
        &git_dir,
        Path::new(common_text.trim()),
    ))
    .map_err(|error| format!("cannot resolve linked-worktree common directory: {error}"))?;
    if common_dir.file_name().and_then(|name| name.to_str()) != Some(".git") {
        return Err("linked worktree common directory is not a .git directory".into());
    }
    let primary = common_dir
        .parent()
        .ok_or_else(|| "linked worktree common directory has no parent".to_owned())?;
    fs::canonicalize(primary).map_err(|error| format!("cannot resolve primary worktree: {error}"))
}

fn resolve_metadata_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn primary_worktree_from_git(repository: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot list Git worktrees: {error}"))?;
    if !output.status.success() {
        return Err("agent-cli must run inside a Git repository".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .ok_or_else(|| "Git did not report a primary worktree".into())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| error.to_string())?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if !names.iter().any(|name| name == column) {
        connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )
            .map_err(|error| format!("cannot migrate {table}.{column}: {error}"))?;
    }
    Ok(())
}

fn migrate_common_dir_database(repository: &Path, root: &Path) -> Result<(), String> {
    let destination = root.join("agent.sqlite3");
    if destination.exists() {
        return Ok(());
    }
    let legacy = Evidence::legacy_common_dir(repository)?.join("agent.sqlite3");
    if !legacy.is_file() {
        return Ok(());
    }
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    fs::copy(&legacy, &destination)
        .map_err(|error| format!("cannot migrate legacy audit database: {error}"))?;
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{EventKind, ProgressEvent};
    use std::ffi::OsString;
    use std::thread;

    fn temporary_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agent-cli-{name}-{}", std::process::id()))
    }

    #[test]
    fn resolves_primary_worktree_from_git_directory() {
        let root = temporary_root("primary-worktree");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("nested/path")).unwrap();

        assert_eq!(
            primary_worktree_from_metadata(&root.join("nested/path")).unwrap(),
            fs::canonicalize(&root).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_linked_worktree_with_relative_metadata_paths() {
        let root = temporary_root("linked-worktree-relative");
        let _ = fs::remove_dir_all(&root);
        let primary = root.join("primary");
        let linked = root.join("linked");
        let git_dir = primary.join(".git/worktrees/linked");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(linked.join("nested")).unwrap();
        fs::write(
            linked.join(".git"),
            "gitdir: ../primary/.git/worktrees/linked\n",
        )
        .unwrap();
        fs::write(git_dir.join("commondir"), "../..\n").unwrap();

        assert_eq!(
            primary_worktree_from_metadata(&linked.join("nested")).unwrap(),
            fs::canonicalize(&primary).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_linked_worktree_with_absolute_common_directory() {
        let root = temporary_root("linked-worktree-absolute");
        let _ = fs::remove_dir_all(&root);
        let primary = root.join("primary");
        let linked = root.join("linked");
        let git_dir = primary.join(".git/worktrees/linked");
        fs::create_dir_all(&git_dir).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        fs::write(
            git_dir.join("commondir"),
            format!("{}\n", primary.join(".git").display()),
        )
        .unwrap();

        assert_eq!(
            primary_worktree_from_metadata(&linked).unwrap(),
            fs::canonicalize(&primary).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn falls_back_to_git_for_separate_git_directory() {
        let root = temporary_root("separate-git-dir");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let worktree = root.join("worktree");
        let git_dir = root.join("storage");
        let output = Command::new("git")
            .args(["init", "-q", "--separate-git-dir"])
            .arg(&git_dir)
            .arg(&worktree)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(primary_worktree_from_metadata(&worktree).is_err());
        assert_eq!(
            primary_worktree(&worktree).unwrap(),
            fs::canonicalize(&git_dir).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn records_malformed_requests_and_retains_metadata_after_log_pruning() {
        let root = temporary_root("audit");
        let evidence = Evidence::open_at(&root).unwrap();
        let request = RawRequest::capture([OsString::from("agent-cli"), OsString::from("bad")]);
        evidence.begin_request(&request).unwrap();
        evidence
            .reject_parse(&request.id, "unknown command")
            .unwrap();
        fs::write(root.join("logs/output.log"), "detail").unwrap();
        assert_eq!(evidence.prune_logs().unwrap(), 1);
        let detail = evidence.run_detail(&request.id).unwrap().unwrap();
        assert_eq!(detail.outcome.as_deref(), Some("rejected"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_task_completion_columns_and_tracks_manual_session() {
        let root = temporary_root("task-migration");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let connection = Connection::open(root.join("agent.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (task_id TEXT PRIMARY KEY, worktree TEXT NOT NULL, created_ms INTEGER NOT NULL, baseline_json TEXT NOT NULL);",
            )
            .unwrap();
        drop(connection);

        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/manual-worktree");
        evidence
            .save_task_baseline("task-one", worktree, &serde_json::json!({}), false)
            .unwrap();
        assert_eq!(
            evidence.active_manual_task_id(worktree).unwrap(),
            Some("task-one".into())
        );
        evidence.close_task("task-one", "abc123").unwrap();
        assert_eq!(evidence.active_manual_task_id(worktree).unwrap(), None);
        let second = evidence
            .save_task_baseline("task-one", worktree, &serde_json::json!({}), false)
            .unwrap();
        assert_eq!(second, "task-one::g2");
        assert_eq!(
            evidence
                .active_task_id_for_session(worktree, "task-one")
                .unwrap(),
            Some(second)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_retains_independent_task_lifecycles() {
        let root = temporary_root("task-generations");
        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/task-generations-worktree");
        let first = evidence
            .save_task_baseline("thread-one", worktree, &"first", false)
            .unwrap();
        assert_eq!(first, "thread-one");
        assert!(evidence
            .save_task_baseline("thread-one", worktree, &"duplicate", false)
            .unwrap_err()
            .contains("lifecycle already active"));
        evidence.close_task(&first, "commit-one").unwrap();

        let second = evidence
            .save_task_baseline("thread-one", worktree, &"second", false)
            .unwrap();
        assert_eq!(second, "thread-one::g2");
        assert_eq!(
            evidence
                .active_task_id_for_session(worktree, "thread-one")
                .unwrap(),
            Some(second)
        );
        assert_eq!(
            evidence
                .latest_committed_task_for_session(worktree, "thread-one")
                .unwrap(),
            Some((first, "commit-one".into()))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delivery_state_is_resumable_and_attestation_is_exact() {
        let root = temporary_root("delivery");
        let _ = fs::remove_dir_all(&root);
        let evidence = Evidence::open_at(&root).unwrap();
        evidence
            .save_task_baseline("task-delivery", Path::new("/tmp/worktree"), &(), false)
            .unwrap();
        let delivery = DeliveryRecord {
            task_id: "task-delivery".into(),
            worktree: PathBuf::from("/tmp/worktree"),
            source_tree: "tree-1".into(),
            commit_sha: Some("commit-1".into()),
            impact: DeploymentImpact::Platform,
            state: DeliveryState::ExternalPending,
            requirement_id: Some("github-actions.rbf-build".into()),
            detail: None,
        };
        evidence.save_delivery(&delivery).unwrap();
        assert_eq!(evidence.delivery("task-delivery").unwrap(), Some(delivery));
        assert!(evidence
            .attest_delivery(
                "task-delivery",
                "github-actions.rbf-build",
                "platform-bundle.yml",
                "wrong-branch",
                "commit-1",
                "{}",
            )
            .is_err());
        assert!(evidence
            .attest_delivery(
                "task-delivery",
                "github-actions.rbf-build",
                "platform-bundle.yml",
                "main",
                "wrong-commit",
                "{}",
            )
            .is_err());
        evidence
            .attest_delivery(
                "task-delivery",
                "github-actions.rbf-build",
                "platform-bundle.yml",
                "main",
                "commit-1",
                "{}",
            )
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn latest_committed_task_excludes_active_tasks() {
        let root = temporary_root("latest-commit");
        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/worktree");
        evidence
            .save_task_baseline("task-committed", worktree, &(), false)
            .unwrap();
        evidence.close_task("task-committed", "abc123").unwrap();
        evidence
            .save_task_baseline("task-active", worktree, &(), false)
            .unwrap();
        assert_eq!(
            evidence.latest_committed_task(worktree).unwrap(),
            Some(("task-committed".into(), "abc123".into()))
        );
    }

    #[test]
    fn superseding_a_task_closes_it_without_fabricating_a_commit() {
        let root = temporary_root("task-supersede");
        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/task-supersede-worktree");
        evidence
            .save_task_baseline("stale-task", worktree, &(), false)
            .unwrap();
        evidence.supersede_task(worktree, "stale-task").unwrap();
        assert_eq!(
            evidence
                .active_task_id_for_session(worktree, "stale-task")
                .unwrap(),
            None
        );
        assert_eq!(evidence.latest_committed_task(worktree).unwrap(), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_scope_uses_immutable_commit_attempt_paths() {
        let root = temporary_root("committed-scope");
        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/committed-scope-worktree");
        evidence
            .save_task_baseline("task-committed", worktree, &"baseline", false)
            .unwrap();
        let request = RawRequest::capture([OsString::from("agent-cli")]);
        evidence.begin_request(&request).unwrap();
        evidence
            .begin_commit_attempt(&request.id, "task-committed", "message")
            .unwrap();
        evidence.close_task("task-committed", "abc123").unwrap();
        evidence
            .update_commit_attempt(
                &request.id,
                &[PathBuf::from("apps/mister/src/main.rs")],
                "committed",
                Some("abc123"),
                Some("message"),
                None,
            )
            .unwrap();

        assert_eq!(
            evidence.latest_committed_scope(worktree).unwrap(),
            Some(CommittedTask {
                task_id: "task-committed".into(),
                commit_sha: "abc123".into(),
                paths: vec![PathBuf::from("apps/mister/src/main.rs")],
            })
        );
        assert_eq!(
            evidence
                .load_task_baseline::<String>("task-committed")
                .unwrap()
                .unwrap()
                .1,
            "baseline"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_scope_rejects_missing_or_mismatched_attempts() {
        let root = temporary_root("committed-scope-corrupt");
        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/committed-scope-corrupt-worktree");
        evidence
            .save_task_baseline("task-committed", worktree, &(), false)
            .unwrap();
        evidence.close_task("task-committed", "abc123").unwrap();
        assert!(evidence
            .latest_committed_scope(worktree)
            .unwrap_err()
            .starts_with("commit_evidence_corrupt:"));

        let request = RawRequest::capture([OsString::from("agent-cli")]);
        evidence.begin_request(&request).unwrap();
        evidence
            .begin_commit_attempt(&request.id, "task-committed", "message")
            .unwrap();
        evidence
            .update_commit_attempt(
                &request.id,
                &[PathBuf::from("apps/mister/src/main.rs")],
                "committed",
                Some("different-sha"),
                Some("message"),
                None,
            )
            .unwrap();
        assert!(evidence
            .latest_committed_scope(worktree)
            .unwrap_err()
            .starts_with("commit_evidence_corrupt:"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn updating_baseline_preserves_task_metadata_and_claims() {
        let root = temporary_root("baseline-update");
        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/baseline-update-worktree");
        evidence
            .save_task_baseline("task-update", worktree, &"before", false)
            .unwrap();
        evidence
            .claim_task_paths("task-update", &[PathBuf::from("owned.txt")])
            .unwrap();
        let created_ms: i64 = evidence
            .connection
            .query_row(
                "SELECT created_ms FROM tasks WHERE task_id='task-update'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        evidence
            .update_task_baseline("task-update", worktree, &"after")
            .unwrap();

        let row: (i64, Option<i64>, Option<String>) = evidence
            .connection
            .query_row(
                "SELECT created_ms, closed_ms, commit_sha FROM tasks WHERE task_id='task-update'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (created_ms, None, None));
        assert_eq!(
            evidence.task_claims("task-update").unwrap(),
            [PathBuf::from("owned.txt")]
        );
        assert_eq!(
            evidence
                .load_task_baseline::<String>("task-update")
                .unwrap()
                .unwrap()
                .1,
            "after"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn separate_connections_write_to_one_wal_database() {
        let root = temporary_root("concurrent");
        Evidence::open_at(&root).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|index| {
                let root = root.clone();
                thread::spawn(move || {
                    let evidence = Evidence::open_at(&root).unwrap();
                    let mut request = RawRequest::capture([OsString::from("agent-cli")]);
                    request.id.push_str(&format!("-{index}"));
                    evidence.begin_request(&request).unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            Evidence::open_at(&root).unwrap().status().unwrap().requests,
            4
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_detail_contains_child_commands_and_progress_events() {
        let root = temporary_root("detail");
        let evidence = Evidence::open_at(&root).unwrap();
        let request = RawRequest::capture([OsString::from("agent-cli")]);
        evidence.begin_request(&request).unwrap();
        let command = evidence
            .begin_command(&request.id, "check.one", "true", &[], None)
            .unwrap();
        evidence.finish_command(command, now_ms(), 0).unwrap();
        evidence
            .record_event(&ProgressEvent {
                v: 1,
                kind: EventKind::Completed,
                run: request.id.clone(),
                seq: 0,
                elapsed_ms: 7,
                phase: "done".into(),
                message: "Passed".into(),
                percent: Some(100),
            })
            .unwrap();

        let detail = evidence.run_detail(&request.id).unwrap().unwrap();
        assert_eq!(detail.commands.len(), 1);
        assert_eq!(detail.commands[0].duration_ms, Some(0));
        assert_eq!(detail.events.len(), 1);
        assert_eq!(detail.events[0].message, "Passed");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn operation_cache_requires_exact_task_operation_and_fingerprint() {
        let root = temporary_root("operation-cache");
        let evidence = Evidence::open_at(&root).unwrap();
        assert!(!evidence
            .has_cached_operation("task-a", "check.one", "fingerprint-a")
            .unwrap());
        evidence
            .cache_operation("task-a", "check.one", "fingerprint-a")
            .unwrap();
        assert!(evidence
            .has_cached_operation("task-a", "check.one", "fingerprint-a")
            .unwrap());
        assert!(!evidence
            .has_cached_operation("task-a", "check.one", "fingerprint-b")
            .unwrap());
        assert!(!evidence
            .has_cached_operation("task-b", "check.one", "fingerprint-a")
            .unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn closing_task_prunes_cache_and_request_timing_includes_bootstrap() {
        let root = temporary_root("cache-pruning-and-timing");
        let _ = fs::remove_dir_all(&root);
        let evidence = Evidence::open_at(&root).unwrap();
        evidence
            .save_task_baseline("task-timed", Path::new("/tmp/worktree"), &(), false)
            .unwrap();
        evidence
            .cache_operation("task-timed", "check.one", "fingerprint")
            .unwrap();

        let request = RawRequest::capture([OsString::from("agent-cli"), OsString::from("check")]);
        thread::sleep(std::time::Duration::from_millis(5));
        evidence.begin_request(&request).unwrap();
        evidence
            .record_intent(
                &request.id,
                &Intent::Check {
                    scope: crate::model::Scope::WorkingTree,
                },
            )
            .unwrap();
        evidence.finish(&request.id, Outcome::Passed).unwrap();
        let (bootstrap_ms, execution_ms): (i64, i64) = evidence
            .connection
            .query_row(
                "SELECT bootstrap_ms, execution_ms FROM requests WHERE id=?1",
                [&request.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(bootstrap_ms >= 5);
        assert!(execution_ms >= 0);

        evidence.close_task("task-timed", "abc123").unwrap();
        assert!(!evidence
            .has_cached_operation("task-timed", "check.one", "fingerprint")
            .unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
