// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::Outcome;
use crate::progress::{FailureEvidence, ProgressEvent};
use crate::request::RawRequest;
use rusqlite::backup::Backup;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
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
    rejection_reason TEXT,
    outcome TEXT,
    bootstrap_ms INTEGER,
    execution_started_ms INTEGER,
    execution_ms INTEGER,
    cohort_id INTEGER,
    parent_request_id TEXT,
    git_sha TEXT,
    queue_ms INTEGER NOT NULL DEFAULT 0
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
    log_path TEXT,
    resource_class TEXT,
    cache_decision TEXT,
    owner_request_id TEXT
);
CREATE TABLE IF NOT EXISTS events (
    request_id TEXT NOT NULL REFERENCES requests(id),
    sequence INTEGER NOT NULL,
    elapsed_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    phase TEXT NOT NULL,
    message TEXT NOT NULL,
    percent INTEGER,
    failure_json TEXT,
    PRIMARY KEY (request_id, sequence)
);
PRAGMA user_version = 12;
"#;

#[derive(Debug)]
pub struct Evidence {
    connection: Connection,
    root: PathBuf,
    git_sha: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DatabaseReport {
    pub requests: i64,
    pub commands: i64,
    pub wall_ms: i64,
    pub cache_hits: i64,
    pub cache_misses: i64,
    pub failures: i64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureEvidence>,
}

impl Evidence {
    pub fn open_for_repository(repository: &Path) -> Result<Self, String> {
        if let Some(root) = std::env::var_os("MISTER_AGENT_CLI_STATE_DIR") {
            return Self::open_at(Path::new(&root));
        }
        let primary = primary_worktree(repository)?;
        let root = primary.join(".agent-cli");
        let mut evidence = Self::open_at(&root)?;
        evidence.git_sha = crate::git::value(repository, &["rev-parse", "HEAD"])
            .unwrap_or_else(|_| "unknown".into());
        Ok(evidence)
    }

    pub fn open_at(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|error| {
            format!("cannot create audit directory {}: {error}", root.display())
        })?;
        let _migration_lock = EvidenceMigrationLock::acquire(&root.join("evidence.lock"))?;
        let database = root.join("agent.sqlite3");
        let connection = Connection::open(&database)
            .map_err(|error| format!("cannot open audit database: {error}"))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| format!("cannot configure audit database: {error}"))?;
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(|error| format!("cannot read audit schema version: {error}"))?;
        match version {
            0 => connection
                .execute_batch(SCHEMA)
                .map_err(|error| format!("cannot initialize audit database: {error}"))?,
            10 => {
                migrate_v10_to_v11(&connection, root)?;
                migrate_v11_to_v12(&connection)?;
            }
            11 => migrate_v11_to_v12(&connection)?,
            12 => {}
            unknown => {
                return Err(format!(
                    "unsupported audit schema version {unknown}; expected 10, 11, or 12"
                ));
            }
        }
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .and_then(|()| connection.pragma_update(None, "foreign_keys", true))
            .and_then(|()| connection.execute_batch(SCHEMA))
            .map_err(|error| format!("cannot configure audit database: {error}"))?;
        fs::create_dir_all(root.join("logs"))
            .map_err(|error| format!("cannot create audit log directory: {error}"))?;
        retain_recent_evidence(&connection)?;
        Ok(Self {
            connection,
            root: root.to_path_buf(),
            git_sha: "unknown".into(),
        })
    }

    pub fn begin_request(&self, request: &RawRequest) -> Result<(), String> {
        let args = serde_json::to_string(&request.args).map_err(|error| error.to_string())?;
        let parent = std::env::var("MISTER_AGENT_PARENT_REQUEST_ID").ok();
        self.connection
            .execute(
                "INSERT INTO requests (id, started_ms, args_json, parent_request_id, git_sha) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![request.id, request.started_ms, args, parent, self.git_sha],
            )
            .map_err(|error| format!("cannot record request: {error}"))?;
        Ok(())
    }

    pub fn record_intent(
        &self,
        request_id: &str,
        intent: &impl serde::Serialize,
    ) -> Result<(), String> {
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
            let failure = event
                .failure
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| format!("cannot serialize progress failure: {error}"))?;
            transaction
                .execute(
                    "INSERT INTO events (request_id, sequence, elapsed_ms, kind, phase, message, percent, failure_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![event.run, i64::from(event.seq), i64::try_from(event.elapsed_ms).unwrap_or(i64::MAX), event.kind.as_str(), event.phase, event.message, event.percent.map(i64::from), failure],
            )
            .map_err(|error| format!("cannot record progress event: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("cannot commit progress events: {error}"))?;
        Ok(())
    }

    pub fn add_queue_ms(&self, request_id: &str, elapsed_ms: i64) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE requests SET queue_ms=queue_ms+?2 WHERE id=?1",
                params![request_id, elapsed_ms],
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
        resource_class: &str,
    ) -> Result<i64, String> {
        let args = serde_json::to_string(args).map_err(|error| error.to_string())?;
        self.connection.execute("INSERT INTO commands (request_id, operation_id, program, args_json, started_ms, status, log_path, resource_class, cache_decision) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, 'miss')", params![request_id, operation_id, program, args, now_ms(), log_path.map(|path| path.display().to_string()), resource_class]).map_err(|error| format!("cannot record command: {error}"))?;
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
        resource_class: &str,
    ) -> Result<(), String> {
        let args = serde_json::to_string(args).map_err(|error| error.to_string())?;
        self.connection.execute(
            "INSERT INTO commands (request_id, operation_id, program, args_json, started_ms, completed_ms, duration_ms, exit_code, status, cache_decision, resource_class) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, 'reused', 'hit', ?6)",
            params![request_id, operation_id, program, args, now_ms(), resource_class],
        ).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn record_joined_command(
        &self,
        request_id: &str,
        owner_request_id: &str,
        operation_id: &str,
        program: &str,
        args: &[String],
        resource_class: &str,
    ) -> Result<(), String> {
        let args = serde_json::to_string(args).map_err(|error| error.to_string())?;
        self.connection.execute(
            "INSERT INTO commands (request_id, operation_id, program, args_json, started_ms, completed_ms, duration_ms, exit_code, status, cache_decision, owner_request_id, resource_class) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 0, 0, 'joined', 'joined_in_flight', ?6, ?7)",
            params![request_id, operation_id, program, args, now_ms(), owner_request_id, resource_class],
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

    pub fn report(&self) -> Result<DatabaseReport, String> {
        let (requests, wall_ms, failures) = self
            .connection
            .query_row(
                "WITH recent AS (
                    SELECT started_ms, completed_ms, outcome
                    FROM requests
                    WHERE parent_request_id IS NULL
                    ORDER BY started_ms DESC
                    LIMIT 200
                 )
                 SELECT count(*),
                        COALESCE(max(completed_ms)-min(started_ms), 0),
                        COALESCE(sum(outcome IN ('failed', 'rejected')), 0)
                 FROM recent",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| error.to_string())?;
        let (commands, cache_hits, cache_misses) = self
            .connection
            .query_row(
                "WITH recent AS (
                    SELECT id
                    FROM requests
                    WHERE parent_request_id IS NULL
                    ORDER BY started_ms DESC
                    LIMIT 200
                 )
                 SELECT count(*),
                        COALESCE(sum(status IN ('reused', 'joined')), 0),
                        COALESCE(sum(cache_decision = 'miss'), 0)
                 FROM commands
                 WHERE request_id IN recent",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|error| error.to_string())?;
        Ok(DatabaseReport {
            requests,
            commands,
            wall_ms,
            cache_hits,
            cache_misses,
            failures,
        })
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
                .prepare("SELECT sequence, elapsed_ms, kind, phase, message, percent, failure_json FROM events WHERE request_id = ?1 ORDER BY sequence")
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
                        failure: row
                            .get::<_, Option<String>>(6)?
                            .and_then(|value| serde_json::from_str(&value).ok()),
                    })
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
        }
        Ok(detail)
    }
}

struct EvidenceMigrationLock(File);

impl EvidenceMigrationLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|error| format!("cannot open evidence migration lock: {error}"))?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result != 0 {
            return Err(format!(
                "cannot acquire evidence migration lock: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(file))
    }
}

impl Drop for EvidenceMigrationLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn migrate_v10_to_v11(connection: &Connection, root: &Path) -> Result<(), String> {
    let backup_path = root.join(format!("agent-v10-backup-{}.sqlite3", now_ms()));
    let mut destination = Connection::open(&backup_path)
        .map_err(|error| format!("cannot create v10 evidence backup: {error}"))?;
    Backup::new(connection, &mut destination)
        .and_then(|backup| backup.run_to_completion(64, std::time::Duration::from_millis(10), None))
        .map_err(|error| format!("cannot back up v10 evidence database: {error}"))?;
    drop(destination);
    let migration = connection.execute_batch(
        "BEGIN EXCLUSIVE;
         DROP TABLE IF EXISTS cohort_summaries;
         DROP TABLE IF EXISTS cohorts;
         PRAGMA user_version = 11;
         COMMIT;",
    );
    if let Err(error) = migration {
        let _ = connection.execute_batch("ROLLBACK;");
        return Err(format!("cannot migrate evidence database to v11: {error}"));
    }
    Ok(())
}

fn migrate_v11_to_v12(connection: &Connection) -> Result<(), String> {
    let has_failure_json = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('events') WHERE name = 'failure_json')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| format!("cannot inspect v11 evidence events: {error}"))?;
    if has_failure_json {
        connection
            .pragma_update(None, "user_version", 12)
            .map_err(|error| format!("cannot finalize evidence database v12: {error}"))?;
        return Ok(());
    }
    let migration = connection.execute_batch(
        "BEGIN IMMEDIATE;
         ALTER TABLE events ADD COLUMN failure_json TEXT;
         PRAGMA user_version = 12;
         COMMIT;",
    );
    if let Err(error) = migration {
        let _ = connection.execute_batch("ROLLBACK;");
        return Err(format!("cannot migrate evidence database to v12: {error}"));
    }
    Ok(())
}

fn retain_recent_evidence(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "DELETE FROM events WHERE request_id IN (
                 SELECT id FROM requests
                 WHERE id NOT IN (SELECT id FROM requests ORDER BY started_ms DESC LIMIT 20000)
             );
             DELETE FROM commands WHERE request_id IN (
                 SELECT id FROM requests
                 WHERE id NOT IN (SELECT id FROM requests ORDER BY started_ms DESC LIMIT 20000)
             );
             DELETE FROM requests
             WHERE id NOT IN (SELECT id FROM requests ORDER BY started_ms DESC LIMIT 20000);",
        )
        .map_err(|error| format!("cannot bound workflow telemetry: {error}"))
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
    fn records_malformed_requests_and_retains_metadata() {
        let root = temporary_root("audit");
        let evidence = Evidence::open_at(&root).unwrap();
        let request = RawRequest::capture([OsString::from("agent-cli"), OsString::from("bad")]);
        evidence.begin_request(&request).unwrap();
        evidence
            .reject_parse(&request.id, "unknown command")
            .unwrap();
        let detail = evidence.run_detail(&request.id).unwrap().unwrap();
        assert_eq!(detail.outcome.as_deref(), Some("rejected"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_schema_versions_are_rejected_without_modification() {
        let root = temporary_root("legacy-ownership");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let connection = Connection::open(root.join("agent.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    task_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    worktree TEXT NOT NULL,
                    created_ms INTEGER NOT NULL,
                    baseline_json TEXT NOT NULL,
                    closed_ms INTEGER,
                    commit_sha TEXT
                );
                CREATE TABLE task_claims (
                    task_id TEXT NOT NULL,
                    path TEXT NOT NULL,
                    claimed_ms INTEGER NOT NULL,
                    PRIMARY KEY (task_id, path)
                );
                INSERT INTO tasks
                    (task_id, session_id, generation, worktree, created_ms, baseline_json)
                    VALUES ('legacy-task', 'legacy-task', 1, '/tmp/worktree', 1, '{}');
                INSERT INTO task_claims (task_id, path, claimed_ms)
                    VALUES ('legacy-task', 'legacy.txt', 1);
                PRAGMA user_version = 9;",
            )
            .unwrap();
        drop(connection);

        let error = Evidence::open_at(&root).unwrap_err();
        assert!(error.contains("unsupported audit schema version 9"));
        let connection = Connection::open(root.join("agent.sqlite3")).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM tasks", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_v10_with_a_recoverable_sqlite_backup() {
        let root = temporary_root("v10-migration");
        let _ = fs::remove_dir_all(&root);
        let evidence = Evidence::open_at(&root).unwrap();
        let request = RawRequest::capture([OsString::from("agent-cli")]);
        evidence.begin_request(&request).unwrap();
        drop(evidence);
        let connection = Connection::open(root.join("agent.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE cohorts (
                    id INTEGER PRIMARY KEY,
                    created_ms INTEGER NOT NULL,
                    git_sha TEXT NOT NULL,
                    planner_schema INTEGER NOT NULL,
                    label TEXT NOT NULL
                 );
                 CREATE TABLE cohort_summaries (
                    archived_ms INTEGER PRIMARY KEY,
                    git_sha TEXT NOT NULL,
                    requests INTEGER NOT NULL,
                    p95_request_ms INTEGER NOT NULL,
                    cache_effectiveness_percent REAL NOT NULL
                 );
                 PRAGMA user_version = 10;",
            )
            .unwrap();
        drop(connection);

        let migrated = Evidence::open_at(&root).unwrap();
        assert_eq!(
            migrated
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            12
        );
        assert_eq!(
            migrated
                .connection
                .query_row("SELECT count(*) FROM requests", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(
            migrated
                .connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='cohorts'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .unwrap()
                .is_none()
        );
        let backup = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("agent-v10-backup-"))
            })
            .expect("v10 backup");
        let backup = Connection::open(backup).unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_v11_events_to_nullable_structured_failures() {
        let root = temporary_root("v11-migration");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let connection = Connection::open(root.join("agent.sqlite3")).unwrap();
        let legacy_schema = SCHEMA
            .replace("    failure_json TEXT,\n", "")
            .replace("PRAGMA user_version = 12;", "PRAGMA user_version = 11;");
        connection.execute_batch(&legacy_schema).unwrap();
        drop(connection);

        let migrated = Evidence::open_at(&root).unwrap();
        assert_eq!(
            migrated
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            12
        );
        assert!(
            migrated
                .connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('events') WHERE name = 'failure_json')",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap()
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
        let evidence = Evidence::open_at(&root).unwrap();
        assert_eq!(
            evidence
                .connection
                .query_row("SELECT count(*) FROM requests", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            4
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn report_is_bounded_to_latest_two_hundred_top_level_requests() {
        let root = temporary_root("bounded-report");
        let _ = fs::remove_dir_all(&root);
        let evidence = Evidence::open_at(&root).unwrap();
        for index in 0..205_i64 {
            evidence
                .connection
                .execute(
                    "INSERT INTO requests (
                        id, started_ms, completed_ms, args_json, parse_status, outcome
                     ) VALUES (?1, ?2, ?3, '[]', 'parsed', ?4)",
                    params![
                        format!("request-{index}"),
                        index,
                        index + 10,
                        if matches!(index, 0 | 204) {
                            "failed"
                        } else {
                            "passed"
                        }
                    ],
                )
                .unwrap();
        }
        evidence
            .connection
            .execute(
                "INSERT INTO requests (
                    id, started_ms, completed_ms, args_json, parse_status, outcome,
                    parent_request_id
                 ) VALUES ('child', 300, 301, '[]', 'parsed', 'failed', 'request-204')",
                [],
            )
            .unwrap();
        for (request, status, cache) in [
            ("request-204", "reused", "hit"),
            ("request-203", "passed", "miss"),
            ("request-0", "failed", "miss"),
        ] {
            evidence
                .connection
                .execute(
                    "INSERT INTO commands (
                        request_id, operation_id, program, args_json, started_ms, status,
                        cache_decision
                     ) VALUES (?1, 'check', 'true', '[]', 0, ?2, ?3)",
                    params![request, status, cache],
                )
                .unwrap();
        }

        let report = evidence.report().unwrap();
        assert_eq!(report.requests, 200);
        assert_eq!(report.commands, 2);
        assert_eq!(report.failures, 1);
        assert_eq!(report.wall_ms, 209);
        assert_eq!(report.cache_hits, 1);
        assert_eq!(report.cache_misses, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_detail_contains_child_commands_and_progress_events() {
        let root = temporary_root("detail");
        let evidence = Evidence::open_at(&root).unwrap();
        let request = RawRequest::capture([OsString::from("agent-cli")]);
        evidence.begin_request(&request).unwrap();
        let command = evidence
            .begin_command(&request.id, "check.one", "true", &[], None, "cpu")
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
                failure: Some(FailureEvidence {
                    code: "artifact_mismatch".into(),
                    phase: "artifact".into(),
                    retry_policy: "reconcile_then_retry".into(),
                    recovery_required: false,
                }),
            })
            .unwrap();

        let detail = evidence.run_detail(&request.id).unwrap().unwrap();
        assert_eq!(detail.commands.len(), 1);
        assert_eq!(detail.commands[0].duration_ms, Some(0));
        assert_eq!(detail.events.len(), 1);
        assert_eq!(detail.events[0].message, "Passed");
        assert_eq!(
            detail.events[0].failure.as_ref().unwrap().code,
            "artifact_mismatch"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn request_timing_includes_bootstrap() {
        let root = temporary_root("request-timing");
        let _ = fs::remove_dir_all(&root);
        let evidence = Evidence::open_at(&root).unwrap();

        let request = RawRequest::capture([OsString::from("agent-cli"), OsString::from("check")]);
        thread::sleep(std::time::Duration::from_millis(5));
        evidence.begin_request(&request).unwrap();
        evidence
            .record_intent(&request.id, &serde_json::json!({"command": "plan"}))
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
        fs::remove_dir_all(root).unwrap();
    }
}
