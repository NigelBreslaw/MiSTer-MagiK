// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

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
    outcome TEXT
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
PRAGMA user_version = 1;
"#;

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
                params![request.id, now_ms(), args],
            )
            .map_err(|error| format!("cannot record request: {error}"))?;
        Ok(())
    }

    pub fn record_intent(&self, request_id: &str, intent: &Intent) -> Result<(), String> {
        let intent = serde_json::to_string(intent).map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "UPDATE requests SET parse_status = 'parsed', intent_json = ?2 WHERE id = ?1",
                params![request_id, intent],
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
                "UPDATE requests SET completed_ms = ?2, outcome = ?3 WHERE id = ?1",
                params![request_id, now_ms(), outcome],
            )
            .map_err(|error| format!("cannot record request outcome: {error}"))?;
        Ok(())
    }

    pub fn record_event(&self, event: &ProgressEvent) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO events (request_id, sequence, elapsed_ms, kind, phase, message, percent) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![event.run, i64::from(event.seq), i64::try_from(event.elapsed_ms).unwrap_or(i64::MAX), event.kind.as_str(), event.phase, event.message, event.percent.map(i64::from)],
            )
            .map_err(|error| format!("cannot record progress event: {error}"))?;
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
}
