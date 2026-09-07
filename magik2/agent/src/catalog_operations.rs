//! Reuse installed production catalog commands, without starting the UI or shell.
use crate::{Agent, Envelope, FrameError, response, write_frame};
use serde_json::json;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn arguments(request: &Envelope) -> Result<(PathBuf, Vec<String>), String> {
    let fields = &request.fields;
    let root = match fields.get("layout").and_then(|v| v.as_str()) {
        Some("dev") => "/media/fat/mister-magik-dev",
        Some("public") => "/media/fat/mister-magik",
        _ => return Err("catalog layout must be dev or public".into()),
    };
    let action = fields
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or("catalog action required")?;
    let command = match action {
        "inspect" => "catalog-inspect",
        "metadata-qualification" => "metadata-qualification-report",
        "rom-audit" => "catalog-arcade-rom-audit",
        "neogeo-family-audit" => "catalog-neogeo-family-audit",
        "screenshots" => "catalog-screenshot-audit",
        "preview-render" => "preview-render-probe",
        "purge"
            if fields.get("layout").and_then(|v| v.as_str()) == Some("dev")
                && fields.get("confirm").and_then(|v| v.as_bool()) == Some(true) =>
        {
            "purge-library-data"
        }
        _ => return Err("unsupported catalog action".into()),
    };
    let mut args = vec![command.to_owned()];
    let expected = if action == "purge" {
        args.push("--confirm".into());
        3
    } else if matches!(action, "screenshots" | "preview-render") {
        let system = fields
            .get("system")
            .and_then(|v| v.as_str())
            .ok_or("screenshot system required")?;
        if system.is_empty()
            || system.len() > 48
            || !system
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err("invalid screenshot system".into());
        }
        args.push(system.into());
        if action == "preview-render" {
            let key = fields
                .get("asset_key")
                .and_then(|v| v.as_str())
                .ok_or("asset key required")?;
            if key.is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
                return Err("invalid asset key".into());
            }
            args.push(key.into());
            4
        } else {
            3
        }
    } else {
        2
    };
    if fields.len() != expected {
        return Err("unexpected catalog arguments".into());
    }
    Ok((PathBuf::from(root).join("mister-magik-fb"), args))
}

impl Agent {
    pub(super) fn catalog_operation(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
        body: &[u8],
    ) -> Result<(), FrameError> {
        if request.fields.get("action").and_then(|v| v.as_str()) == Some("cores") {
            let result = if request.fields.len() == 2
                && body.is_empty()
                && matches!(
                    request.fields.get("layout").and_then(|v| v.as_str()),
                    Some("dev" | "public")
                ) {
                core_inventory()
            } else {
                Err("invalid core inventory request".into())
            };
            let (code, error, output) = match result {
                Ok(output) => (0, None, output),
                Err(error) => (2, Some(error), Vec::new()),
            };
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "catalog-result",
                    json!({"exit_code":code,"error":error,"format":"json"}),
                ),
                &output,
            );
        }
        if request.fields.get("action").and_then(|v| v.as_str()) == Some("query") {
            let result = (|| {
                if !body.is_empty() || request.fields.len() != 4 {
                    return Err("invalid query arguments".into());
                }
                let root = match request.fields.get("layout").and_then(|v| v.as_str()) {
                    Some("dev") => "/media/fat/mister-magik-dev",
                    Some("public") => "/media/fat/mister-magik",
                    _ => return Err("invalid catalog layout".into()),
                };
                let database = request
                    .fields
                    .get("database")
                    .and_then(|v| v.as_str())
                    .ok_or("database required")?;
                let sql = request
                    .fields
                    .get("sql")
                    .and_then(|v| v.as_str())
                    .ok_or("query required")?;
                query(std::path::Path::new(root), database, sql)
            })();
            let (code, error, output) = match result {
                Ok(output) => (0, None, output),
                Err(error) => (2, Some(error), Vec::new()),
            };
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "catalog-result",
                    json!({"exit_code":code,"error":error,"format":"tsv","layout":request.fields["layout"]}),
                ),
                &output,
            );
        }
        let parsed = arguments(request).and_then(|value| {
            if body.is_empty() {
                Ok(value)
            } else {
                Err("catalog request has unexpected body".into())
            }
        });
        let (binary, args) = match parsed {
            Ok(value) => value,
            Err(error) => {
                return write_frame(
                    stream,
                    &response(
                        &request.id,
                        "error",
                        json!({"code":"invalid-catalog-request","detail":error}),
                    ),
                    &[],
                );
            }
        };
        let purge = request.fields.get("action").and_then(|v| v.as_str()) == Some("purge");
        let _mutation = self.mutations.lock().expect("mutation state poisoned");
        if purge {
            self.stop_owned_process().map_err(FrameError::Io)?;
            if let Err(error) = crate::main_control::handoff("mister_magik_suspend\n") {
                let restored = crate::main_control::handoff("mister_magik_resume\n");
                return write_frame(
                    stream,
                    &response(
                        &request.id,
                        "error",
                        json!({"code":"purge-suspend-failed","detail":format!("{error}; Main restoration: {restored:?}")}),
                    ),
                    &[],
                );
            }
        }
        let mut command = Command::new(&binary);
        command.args(args).stdin(std::process::Stdio::null());
        let output = crate::benchmark::execute_bounded(
            &mut command,
            Duration::from_secs(120),
            || {
                let mut byte = [0];
                stream.set_nonblocking(true).ok();
                let disconnected = stream.peek(&mut byte).is_ok_and(|n| n == 0);
                stream.set_nonblocking(false).ok();
                disconnected
            },
            16 * 1024 * 1024,
        );
        let restoration = if purge {
            crate::main_control::handoff("mister_magik_resume\n").err()
        } else {
            None
        };
        let marker_error = (purge
            && output.code == Some(0)
            && !String::from_utf8_lossy(&output.stdout).contains("purge_library_data\tdone\t"))
        .then(|| "purge completion marker absent".to_owned());
        let error = output.error.or(restoration).or(marker_error);
        let header = response(
            &request.id,
            "catalog-result",
            json!({
                "layout":request.fields["layout"],"action":request.fields["action"],
                "executable":binary,"exit_code":output.code,"error":error,
                "stderr":String::from_utf8_lossy(&output.stderr),"format":"production-report",
                "legacy_sqlite_absence":legacy_sqlite_absence(),
            }),
        );
        write_frame(stream, &header, &output.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_rejects_mutation_attachment_and_escaped_storage() {
        let root = std::env::temp_dir().join(format!("magik2-query-{}", std::process::id()));
        let state = root.join("catalog-fast-v1/state");
        std::fs::create_dir_all(&state).unwrap();
        let path = state.join("catalog-state.sqlite3");
        let database = rusqlite::Connection::open(&path).unwrap();
        database
            .execute_batch("CREATE TABLE games (name TEXT); INSERT INTO games VALUES ('Arcade');")
            .unwrap();
        drop(database);
        assert_eq!(
            String::from_utf8(query(&root, "registry", "SELECT name FROM games").unwrap()).unwrap(),
            "name\nArcade\n"
        );
        for sql in [
            "DELETE FROM games",
            "ATTACH ':memory:' AS extra",
            "PRAGMA writable_schema=ON",
        ] {
            assert!(query(&root, "registry", sql).is_err(), "{sql}");
        }
        std::fs::remove_file(&path).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
            assert!(
                query(&root, "registry", "SELECT 1")
                    .unwrap_err()
                    .contains("escapes")
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fixed_executable_and_closed_arguments() {
        let request = Envelope {
            id: "x".into(),
            op: "catalog-operation".into(),
            token: "".into(),
            fields: json!({"layout":"dev","action":"inspect"})
                .as_object()
                .unwrap()
                .clone(),
        };
        let (path, args) = arguments(&request).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/media/fat/mister-magik-dev/mister-magik-fb")
        );
        assert_eq!(args, vec!["catalog-inspect"]);
        for fields in [
            json!({"layout":"../","action":"inspect"}),
            json!({"layout":"dev","action":"shell"}),
            json!({"layout":"dev","action":"screenshots","system":"../x"}),
        ] {
            let request = Envelope {
                fields: fields.as_object().unwrap().clone(),
                ..request.clone()
            };
            assert!(arguments(&request).is_err());
        }
    }
}

fn query(root: &std::path::Path, database: &str, sql: &str) -> Result<Vec<u8>, String> {
    use mister_magik_catalog::{shard_registry, sqlite_inspect};
    use rusqlite::{Connection, OpenFlags};
    use std::time::Instant;
    let storage = root.join("catalog-fast-v1");
    let path = match database {
        "registry" => storage.join("state/catalog-state.sqlite3"),
        "library" => storage.join("state/scanner-cache.sqlite3"),
        value if value.starts_with("system:") => {
            let id = &value[7..];
            let manifest = shard_registry::read_latest_manifest_lazy(
                &storage,
                shard_registry::production_registry_limits(),
            )
            .map_err(|e| e.to_string())?;
            let relative = manifest
                .systems
                .into_iter()
                .find(|system| system.system_id.as_str() == id)
                .and_then(|system| system.active.sqlite_path)
                .ok_or("system has no SQLite database")?;
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err("invalid catalog database path".into());
            }
            storage.join(relative)
        }
        _ => return Err("database must be registry, library or system:ID".into()),
    };
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(storage.canonicalize().map_err(|e| e.to_string())?) {
        return Err("database escapes catalog storage".into());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(1))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    connection
        .progress_handler(1000, Some(move || Instant::now() >= deadline))
        .map_err(|e| e.to_string())?;
    connection
        .authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
            use rusqlite::hooks::{AuthAction, Authorization};
            match context.action {
                AuthAction::Read { .. }
                | AuthAction::Select
                | AuthAction::Recursive
                | AuthAction::Pragma { .. } => Authorization::Allow,
                AuthAction::Function { function_name } if function_name != "load_extension" => {
                    Authorization::Allow
                }
                _ => Authorization::Deny,
            }
        }))
        .map_err(|e| e.to_string())?;
    let mut statement = connection.prepare(sql).map_err(|e| e.to_string())?;
    if !sqlite_inspect::sqlite_statement_is_inspect_only(sql, &statement) {
        return Err("query must be read-only SELECT or inspection PRAGMA".into());
    }
    let columns = statement.column_count();
    let mut text = statement.column_names().join("\t") + "\n";
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        for index in 0..columns {
            if index > 0 {
                text.push('\t');
            }
            text.push_str(&sqlite_inspect::tsv_field(
                &sqlite_inspect::sqlite_cell_to_string(row, index).map_err(|e| e.to_string())?,
            ));
        }
        text.push('\n');
        if text.len() > 16 * 1024 * 1024 || Instant::now() >= deadline {
            return Err("catalog query exceeded its output or five-second limit".into());
        }
    }
    Ok(text.into_bytes())
}

fn core_inventory() -> Result<Vec<u8>, String> {
    fn walk(
        path: &std::path::Path,
        depth: u8,
        count: &mut usize,
        out: &mut Vec<serde_json::Value>,
    ) -> Result<(), String> {
        let entries = match std::fs::read_dir(path) {
            Ok(value) => value,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            *count += 1;
            if *count > 10000 {
                return Err("core inventory exceeds 10000 entries".into());
            }
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() && depth > 0 {
                walk(&entry.path(), depth - 1, count, out)?;
            } else if kind.is_file() && entry.path().extension().is_some_and(|v| v == "rbf") {
                out.push(json!({"path":entry.path(),"bytes":entry.metadata().map_err(|e|e.to_string())?.len()}));
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    let mut count = 0;
    for root in ["_Console", "_Computer", "_Arcade/cores", "_LLAPI"] {
        walk(
            &std::path::Path::new("/media/fat").join(root),
            3,
            &mut count,
            &mut entries,
        )?;
    }
    entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    serde_json::to_vec(&json!({"cores":entries})).map_err(|e| e.to_string())
}

fn legacy_sqlite_absence() -> serde_json::Value {
    let paths: Vec<_> = [
        "/media/fat/mister-magik/mame.sqlite3",
        "/media/fat/mister-magik/hbmame.sqlite3",
        "/media/fat/mister-magik-dev/mame.sqlite3",
        "/media/fat/mister-magik-dev/hbmame.sqlite3",
    ]
    .iter()
    .map(|path| match std::fs::symlink_metadata(path) {
        Ok(_) => json!({"path":path,"present":true}),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"path":path,"present":false}),
        Err(e) => json!({"path":path,"error":e.to_string()}),
    })
    .collect();
    json!({"all_absent":paths.iter().all(|entry|entry["present"]==false),"paths":paths})
}
