// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::model::{Intent, Outcome};
use crate::progress::ProgressEvent;
use crate::request::RawRequest;
use rusqlite::{Connection, OptionalExtension, params};
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
    execution_ms INTEGER,
    cohort_id INTEGER,
    parent_request_id TEXT,
    git_sha TEXT,
    planner_schema INTEGER NOT NULL DEFAULT 3,
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
    PRIMARY KEY (request_id, sequence)
);
CREATE TABLE IF NOT EXISTS validation_results (
    operation_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result TEXT NOT NULL,
    detail TEXT,
    completed_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL,
    PRIMARY KEY (operation_id, fingerprint)
);
CREATE TABLE IF NOT EXISTS operation_leases (
    operation_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    owner_request_id TEXT NOT NULL,
    acquired_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL,
    PRIMARY KEY (operation_id, fingerprint)
);
CREATE TABLE IF NOT EXISTS cohorts (
    id INTEGER PRIMARY KEY,
    created_ms INTEGER NOT NULL,
    git_sha TEXT NOT NULL,
    planner_schema INTEGER NOT NULL,
    label TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS cohort_summaries (
    archived_ms INTEGER PRIMARY KEY,
    git_sha TEXT NOT NULL,
    requests INTEGER NOT NULL,
    p95_request_ms INTEGER NOT NULL,
    cache_effectiveness_percent REAL NOT NULL
);
PRAGMA user_version = 10;
"#;

#[derive(Debug)]
pub struct Evidence {
    connection: Connection,
    root: PathBuf,
    git_sha: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DatabaseStatus {
    pub path: PathBuf,
    pub requests: i64,
    pub commands: i64,
    pub events: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DatabaseReport {
    pub cohort_id: i64,
    pub requests: i64,
    pub commands: i64,
    pub wall_ms: i64,
    pub command_ms: i64,
    pub critical_path_ms: i64,
    pub p50_request_ms: i64,
    pub p95_request_ms: i64,
    pub cache_hits: i64,
    pub cache_misses: i64,
    pub cache_effectiveness_percent: f64,
    pub failures: i64,
    pub repeated_requests: i64,
    pub previous_p95_request_ms: Option<i64>,
    pub p95_regression_ms: Option<i64>,
    pub delivery_no_op: DeliveryDecisionMetrics,
    pub delivery_runtime: DeliveryDecisionMetrics,
    pub delivery_platform: DeliveryDecisionMetrics,
    pub delivery_phases: Vec<DeliveryPhaseMetrics>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeliveryDecisionMetrics {
    pub requests: i64,
    pub p50_ms: i64,
    pub p95_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeliveryPhaseMetrics {
    pub phase: String,
    pub samples: i64,
    pub total_ms: i64,
    pub average_ms: i64,
    pub max_ms: i64,
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
        let mut evidence = Self::open_at(&root)?;
        evidence.git_sha = crate::git::value(repository, &["rev-parse", "HEAD"])
            .unwrap_or_else(|_| "unknown".into());
        Ok(evidence)
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
            .and_then(|()| connection.busy_timeout(std::time::Duration::from_secs(5)))
            .and_then(|()| connection.pragma_update(None, "foreign_keys", true))
            .and_then(|()| connection.execute_batch(SCHEMA))
            .map_err(|error| format!("cannot migrate audit database: {error}"))?;
        ensure_column(&connection, "requests", "bootstrap_ms", "INTEGER")?;
        ensure_column(&connection, "requests", "execution_started_ms", "INTEGER")?;
        ensure_column(&connection, "requests", "execution_ms", "INTEGER")?;
        ensure_column(&connection, "requests", "cohort_id", "INTEGER")?;
        ensure_column(&connection, "requests", "parent_request_id", "TEXT")?;
        ensure_column(&connection, "requests", "git_sha", "TEXT")?;
        ensure_column(
            &connection,
            "requests",
            "planner_schema",
            "INTEGER NOT NULL DEFAULT 3",
        )?;
        ensure_column(
            &connection,
            "requests",
            "queue_ms",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        ensure_column(&connection, "commands", "resource_class", "TEXT")?;
        ensure_column(&connection, "commands", "cache_decision", "TEXT")?;
        ensure_column(&connection, "commands", "owner_request_id", "TEXT")?;
        connection
            .execute(
                "INSERT OR IGNORE INTO cohorts (id, created_ms, git_sha, planner_schema, label) VALUES (1, ?1, 'legacy', 3, 'legacy')",
                [now_ms()],
            )
            .map_err(|error| format!("cannot initialize evidence cohort: {error}"))?;
        connection
            .execute(
                "UPDATE requests SET cohort_id=1 WHERE cohort_id IS NULL",
                [],
            )
            .map_err(|error| format!("cannot migrate request cohorts: {error}"))?;
        connection
            .execute(
                "DELETE FROM validation_results WHERE expires_ms < ?1 OR rowid NOT IN (SELECT rowid FROM validation_results ORDER BY completed_ms DESC LIMIT 10000)",
                [now_ms()],
            )
            .map_err(|error| format!("cannot prune validation cache: {error}"))?;
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
            .map_err(|error| format!("cannot bound workflow telemetry: {error}"))?;
        Ok(Self {
            connection,
            root: root.to_path_buf(),
            git_sha: "unknown".into(),
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
        let parent = std::env::var("MISTER_AGENT_PARENT_REQUEST_ID").ok();
        let cohort: i64 = self
            .connection
            .query_row("SELECT max(id) FROM cohorts", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "INSERT INTO requests (id, started_ms, args_json, cohort_id, parent_request_id, git_sha) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![request.id, request.started_ms, args, cohort, parent, self.git_sha],
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

    pub fn cached_validation(
        &self,
        operation_id: &str,
        fingerprint: &str,
    ) -> Result<Option<(String, Option<String>)>, String> {
        self.connection
            .query_row(
                "SELECT result, detail FROM validation_results WHERE operation_id=?1 AND fingerprint=?2 AND expires_ms>=?3",
                params![operation_id, fingerprint, now_ms()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn cache_validation(
        &self,
        operation_id: &str,
        fingerprint: &str,
        result: &str,
        detail: Option<&str>,
    ) -> Result<(), String> {
        let lifetime_ms = if result == "passed" {
            30 * 24 * 60 * 60 * 1_000_i64
        } else {
            10 * 60 * 1_000_i64
        };
        let completed = now_ms();
        self.connection
            .execute(
                "INSERT OR REPLACE INTO validation_results (operation_id, fingerprint, result, detail, completed_ms, expires_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![operation_id, fingerprint, result, detail, completed, completed.saturating_add(lifetime_ms)],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn claim_validation(
        &self,
        operation_id: &str,
        fingerprint: &str,
        request_id: &str,
    ) -> Result<bool, String> {
        let now = now_ms();
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM operation_leases WHERE operation_id=?1 AND fingerprint=?2 AND expires_ms<?3",
                params![operation_id, fingerprint, now],
            )
            .map_err(|error| error.to_string())?;
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO operation_leases (operation_id, fingerprint, owner_request_id, acquired_ms, expires_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![operation_id, fingerprint, request_id, now, now.saturating_add(31 * 60 * 1_000)],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(inserted == 1)
    }

    pub fn validation_owner(
        &self,
        operation_id: &str,
        fingerprint: &str,
    ) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT owner_request_id FROM operation_leases WHERE operation_id=?1 AND fingerprint=?2",
                params![operation_id, fingerprint],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn heartbeat_validation(
        &self,
        operation_id: &str,
        fingerprint: &str,
        request_id: &str,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE operation_leases SET expires_ms=?4 WHERE operation_id=?1 AND fingerprint=?2 AND owner_request_id=?3",
                params![
                    operation_id,
                    fingerprint,
                    request_id,
                    now_ms().saturating_add(31 * 60 * 1_000)
                ],
            )
            .map_err(|error| error.to_string())?;
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

    pub fn release_validation(
        &self,
        operation_id: &str,
        fingerprint: &str,
        request_id: &str,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "DELETE FROM operation_leases WHERE operation_id=?1 AND fingerprint=?2 AND owner_request_id=?3",
                params![operation_id, fingerprint, request_id],
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

    pub fn status(&self) -> Result<DatabaseStatus, String> {
        Ok(DatabaseStatus {
            path: self.root.join("agent.sqlite3"),
            requests: self.count("requests")?,
            commands: self.count("commands")?,
            events: self.count("events")?,
        })
    }

    pub fn report(&self) -> Result<DatabaseReport, String> {
        let cohort_id: i64 = self
            .connection
            .query_row("SELECT max(id) FROM cohorts", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let scalar = |sql: &str| {
            self.connection
                .query_row(sql, [cohort_id], |row| row.get::<_, i64>(0))
                .map_err(|error| error.to_string())
        };
        let percentile = |selected_cohort: i64, offset_percent: i64| {
            self.connection
                .query_row(
                    "SELECT COALESCE(execution_ms,0) FROM requests WHERE cohort_id=?1 AND parent_request_id IS NULL AND completed_ms IS NOT NULL ORDER BY COALESCE(execution_ms,0) LIMIT 1 OFFSET MAX(0, ((SELECT count(*) FROM requests WHERE cohort_id=?1 AND parent_request_id IS NULL AND completed_ms IS NOT NULL) * ?2 + 99) / 100 - 1)",
                    params![selected_cohort, offset_percent],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map(|value| value.unwrap_or(0))
                .map_err(|error| error.to_string())
        };
        let previous_p95: Option<i64> = self
            .connection
            .query_row(
                "SELECT p95_request_ms FROM cohort_summaries ORDER BY archived_ms DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let p50 = percentile(cohort_id, 50)?;
        let p95 = percentile(cohort_id, 95)?;
        let cache_hits = scalar(
            "SELECT count(*) FROM commands WHERE request_id IN (SELECT id FROM requests WHERE cohort_id=?1) AND status IN ('reused','joined')",
        )?;
        let cache_misses = scalar(
            "SELECT count(*) FROM commands WHERE request_id IN (SELECT id FROM requests WHERE cohort_id=?1) AND cache_decision='miss'",
        )?;
        let delivery_metrics = |decision: &str| -> Result<DeliveryDecisionMetrics, String> {
            let predicate = "r.cohort_id=?1 AND r.parent_request_id IS NULL AND r.intent_json='\"deliver\"' AND r.completed_ms IS NOT NULL AND ((?2='no-op' AND r.outcome='no_op') OR EXISTS (SELECT 1 FROM events e WHERE e.request_id=r.id AND e.phase='delivery-decision' AND e.message=?2))";
            let requests = self
                .connection
                .query_row(
                    &format!("SELECT count(*) FROM requests r WHERE {predicate}"),
                    params![cohort_id, decision],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| error.to_string())?;
            let percentile = |offset_percent: i64| {
                self.connection
                    .query_row(
                        &format!(
                            "SELECT COALESCE(r.execution_ms,0) FROM requests r WHERE {predicate} ORDER BY COALESCE(r.execution_ms,0) LIMIT 1 OFFSET MAX(0, ((SELECT count(*) FROM requests r WHERE {predicate}) * ?3 + 99) / 100 - 1)"
                        ),
                        params![cohort_id, decision, offset_percent],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map(|value| value.unwrap_or(0))
                    .map_err(|error| error.to_string())
            };
            Ok(DeliveryDecisionMetrics {
                requests,
                p50_ms: percentile(50)?,
                p95_ms: percentile(95)?,
            })
        };
        let delivery_phases = {
            let mut statement = self
                .connection
                .prepare(
                    "WITH ordered AS (
                        SELECT e.request_id, e.phase, e.elapsed_ms,
                               LEAD(e.elapsed_ms) OVER (
                                   PARTITION BY e.request_id ORDER BY e.sequence
                               ) AS next_elapsed
                        FROM events e
                        JOIN requests r ON r.id=e.request_id
                        WHERE r.cohort_id=?1
                          AND r.parent_request_id IS NULL
                          AND r.intent_json='\"deliver\"'
                    )
                    SELECT phase,
                           count(*),
                           COALESCE(sum(MAX(next_elapsed-elapsed_ms,0)),0),
                           COALESCE(CAST(round(avg(MAX(next_elapsed-elapsed_ms,0))) AS INTEGER),0),
                           COALESCE(max(MAX(next_elapsed-elapsed_ms,0)),0)
                    FROM ordered
                    WHERE next_elapsed IS NOT NULL
                      AND phase NOT IN ('request','delivery-decision')
                    GROUP BY phase
                    ORDER BY phase",
                )
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([cohort_id], |row| {
                    Ok(DeliveryPhaseMetrics {
                        phase: row.get(0)?,
                        samples: row.get(1)?,
                        total_ms: row.get(2)?,
                        average_ms: row.get(3)?,
                        max_ms: row.get(4)?,
                    })
                })
                .map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        };
        Ok(DatabaseReport {
            cohort_id,
            requests: scalar("SELECT count(*) FROM requests WHERE cohort_id=?1")?,
            commands: scalar(
                "SELECT count(*) FROM commands WHERE request_id IN (SELECT id FROM requests WHERE cohort_id=?1)",
            )?,
            wall_ms: scalar(
                "SELECT COALESCE(max(completed_ms)-min(started_ms),0) FROM requests WHERE cohort_id=?1",
            )?,
            command_ms: scalar(
                "SELECT COALESCE(sum(duration_ms),0) FROM commands WHERE request_id IN (SELECT id FROM requests WHERE cohort_id=?1)",
            )?,
            critical_path_ms: scalar(
                "SELECT COALESCE(sum(CASE WHEN execution_ms > 0 THEN execution_ms ELSE 0 END),0) FROM requests WHERE cohort_id=?1 AND parent_request_id IS NULL",
            )?,
            p50_request_ms: p50,
            p95_request_ms: p95,
            cache_hits,
            cache_misses,
            cache_effectiveness_percent: if cache_hits + cache_misses == 0 {
                0.0
            } else {
                cache_hits as f64 * 100.0 / (cache_hits + cache_misses) as f64
            },
            failures: scalar(
                "SELECT count(*) FROM commands WHERE request_id IN (SELECT id FROM requests WHERE cohort_id=?1) AND status='failed'",
            )?,
            repeated_requests: scalar(
                "SELECT count(*) FROM requests r WHERE cohort_id=?1 AND EXISTS (SELECT 1 FROM requests p WHERE p.cohort_id=r.cohort_id AND p.id<>r.id AND p.args_json=r.args_json AND p.started_ms BETWEEN r.started_ms-60000 AND r.started_ms)",
            )?,
            previous_p95_request_ms: previous_p95,
            p95_regression_ms: previous_p95.map(|previous| p95.saturating_sub(previous)),
            delivery_no_op: delivery_metrics("no-op")?,
            delivery_runtime: delivery_metrics("runtime")?,
            delivery_platform: delivery_metrics("platform")?,
            delivery_phases,
        })
    }

    pub fn rotate(&self, git_sha: &str) -> Result<PathBuf, String> {
        let summary = self.report()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|error| format!("cannot checkpoint evidence database: {error}"))?;
        let archives = self.root.join("archives");
        fs::create_dir_all(&archives).map_err(|error| error.to_string())?;
        let stamp = now_ms();
        let archive = archives.join(format!("agent-{stamp}-{git_sha}.sqlite3"));
        fs::copy(self.root.join("agent.sqlite3"), &archive)
            .map_err(|error| format!("cannot archive evidence database: {error}"))?;
        let archived = Connection::open(&archive).map_err(|error| error.to_string())?;
        let integrity: String = archived
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if integrity != "ok" {
            let _ = fs::remove_file(&archive);
            return Err(format!(
                "database archive failed integrity check: {integrity}"
            ));
        }
        let archived_logs = archives.join(format!("agent-{stamp}-{git_sha}-logs"));
        if self.root.join("logs").exists() {
            fs::rename(self.root.join("logs"), &archived_logs)
                .map_err(|error| format!("cannot archive evidence logs: {error}"))?;
            fs::create_dir_all(self.root.join("logs")).map_err(|error| error.to_string())?;
        }
        self.connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 DELETE FROM validation_results;
                 DELETE FROM operation_leases;
                 DELETE FROM events;
                 DELETE FROM commands;
                 DELETE FROM requests;
                 DELETE FROM cohorts;
                 INSERT INTO cohorts (created_ms, git_sha, planner_schema, label)
                 VALUES (unixepoch('subsec') * 1000, 'pending', 3, 'post-optimization');
                 COMMIT;",
            )
            .map_err(|error| format!("cannot reset evidence database: {error}"))?;
        self.connection
            .execute(
                "UPDATE cohorts SET git_sha=?1 WHERE id=(SELECT max(id) FROM cohorts)",
                [git_sha],
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "INSERT INTO cohort_summaries (archived_ms, git_sha, requests, p95_request_ms, cache_effectiveness_percent) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    stamp,
                    git_sha,
                    summary.requests,
                    summary.p95_request_ms,
                    summary.cache_effectiveness_percent
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(archive)
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
    fn legacy_ownership_tables_remain_inert() {
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

        let evidence = Evidence::open_at(&root).unwrap();
        assert_eq!(
            evidence
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert_eq!(
            evidence
                .connection
                .query_row("SELECT count(*) FROM tasks", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
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
    fn validation_cache_is_content_scoped_and_leases_recover_after_expiry() {
        let root = temporary_root("validation-cache");
        let evidence = Evidence::open_at(&root).unwrap();
        evidence
            .cache_validation("check.one", "fingerprint", "passed", None)
            .unwrap();
        assert_eq!(
            evidence
                .cached_validation("check.one", "fingerprint")
                .unwrap(),
            Some(("passed".into(), None))
        );
        assert!(
            evidence
                .claim_validation("check.two", "fingerprint", "owner-one")
                .unwrap()
        );
        assert!(
            !evidence
                .claim_validation("check.two", "fingerprint", "owner-two")
                .unwrap()
        );
        evidence
            .connection
            .execute(
                "UPDATE operation_leases SET expires_ms=0 WHERE operation_id='check.two'",
                [],
            )
            .unwrap();
        assert!(
            evidence
                .claim_validation("check.two", "fingerprint", "owner-two")
                .unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn separate_connections_share_one_validation_lease() {
        let root = temporary_root("shared-lease");
        let first = Evidence::open_at(&root).unwrap();
        let second = Evidence::open_at(&root).unwrap();
        assert!(
            first
                .claim_validation("check.one", "fingerprint", "owner")
                .unwrap()
        );
        assert!(
            !second
                .claim_validation("check.one", "fingerprint", "waiter")
                .unwrap()
        );
        first
            .cache_validation("check.one", "fingerprint", "passed", None)
            .unwrap();
        first
            .release_validation("check.one", "fingerprint", "owner")
            .unwrap();
        assert_eq!(
            second
                .cached_validation("check.one", "fingerprint")
                .unwrap(),
            Some(("passed".into(), None))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rotation_archives_integrity_checked_evidence_and_starts_empty() {
        let root = temporary_root("rotation");
        let evidence = Evidence::open_at(&root).unwrap();
        let request = RawRequest::capture([OsString::from("agent-cli"), OsString::from("check")]);
        evidence.begin_request(&request).unwrap();
        evidence.finish(&request.id, Outcome::Passed).unwrap();
        evidence
            .cache_validation("check.one", "fingerprint", "passed", None)
            .unwrap();
        evidence
            .claim_validation("check.two", "fingerprint", "owner")
            .unwrap();
        let archive = evidence.rotate("abc123").unwrap();
        assert!(archive.is_file());
        let archived = Connection::open(archive).unwrap();
        assert_eq!(
            archived
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(evidence.status().unwrap().requests, 0);
        let report = evidence.report().unwrap();
        assert_eq!(report.requests, 0);
        assert!(report.previous_p95_request_ms.is_some());
        assert_eq!(
            evidence
                .connection
                .query_row("SELECT count(*) FROM validation_results", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            evidence
                .connection
                .query_row("SELECT count(*) FROM operation_leases", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn report_groups_delivery_decisions_and_phase_durations() {
        let root = temporary_root("delivery-report");
        let _ = fs::remove_dir_all(&root);
        let evidence = Evidence::open_at(&root).unwrap();
        record_delivery_fixture(&evidence, "noop", "no-op", Outcome::NoOp, 120);
        record_delivery_fixture(&evidence, "runtime", "runtime", Outcome::Passed, 250);
        record_delivery_fixture(&evidence, "platform", "platform", Outcome::Passed, 900);

        let report = evidence.report().unwrap();
        assert_eq!(
            report.delivery_no_op,
            DeliveryDecisionMetrics {
                requests: 1,
                p50_ms: 120,
                p95_ms: 120,
            }
        );
        assert_eq!(report.delivery_runtime.requests, 1);
        assert_eq!(report.delivery_runtime.p95_ms, 250);
        assert_eq!(report.delivery_platform.requests, 1);
        assert_eq!(report.delivery_platform.p95_ms, 900);
        assert_eq!(
            report
                .delivery_phases
                .iter()
                .find(|phase| phase.phase == "reconciliation")
                .unwrap(),
            &DeliveryPhaseMetrics {
                phase: "reconciliation".into(),
                samples: 3,
                total_ms: 60,
                average_ms: 20,
                max_ms: 20,
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn record_delivery_fixture(
        evidence: &Evidence,
        suffix: &str,
        decision: &str,
        outcome: Outcome,
        execution_ms: i64,
    ) {
        let mut request =
            RawRequest::capture([OsString::from("agent-cli"), OsString::from("deliver")]);
        request.id.push_str(suffix);
        evidence.begin_request(&request).unwrap();
        evidence
            .record_intent(&request.id, &Intent::Deliver)
            .unwrap();
        evidence
            .record_events(&[
                ProgressEvent {
                    v: 1,
                    kind: EventKind::Progress,
                    run: request.id.clone(),
                    seq: 0,
                    elapsed_ms: 10,
                    phase: "reconciliation".into(),
                    message: "delivery reconciliation".into(),
                    percent: Some(15),
                },
                ProgressEvent {
                    v: 1,
                    kind: EventKind::Progress,
                    run: request.id.clone(),
                    seq: 1,
                    elapsed_ms: 30,
                    phase: "cleanup".into(),
                    message: "cleaning transient delivery staging".into(),
                    percent: None,
                },
                ProgressEvent {
                    v: 1,
                    kind: EventKind::Completed,
                    run: request.id.clone(),
                    seq: 2,
                    elapsed_ms: 35,
                    phase: "delivery-decision".into(),
                    message: decision.into(),
                    percent: Some(100),
                },
            ])
            .unwrap();
        evidence.finish(&request.id, outcome).unwrap();
        evidence
            .connection
            .execute(
                "UPDATE requests SET execution_ms=?2, parent_request_id=NULL WHERE id=?1",
                params![request.id, execution_ms],
            )
            .unwrap();
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
        fs::remove_dir_all(root).unwrap();
    }
}
