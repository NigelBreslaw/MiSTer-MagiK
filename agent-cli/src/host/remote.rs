// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use ssh2::{ExtendedData, Session};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::Result;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConnectionConfig {
    host: String,
    user: String,
    password: String,
}

impl ConnectionConfig {
    pub(crate) fn from_environment() -> Self {
        Self::from_values(
            host_from(env::var("MISTER_IP").ok().as_deref()),
            env::var("MISTER_USER").ok().as_deref(),
            env::var("MISTER_PASS").ok().as_deref(),
        )
    }

    pub(crate) fn for_resolved_host(host: impl Into<String>) -> Self {
        Self::from_values(
            host,
            env::var("MISTER_USER").ok().as_deref(),
            env::var("MISTER_PASS").ok().as_deref(),
        )
    }

    pub(crate) fn from_values(
        host: impl Into<String>,
        user: Option<&str>,
        password: Option<&str>,
    ) -> Self {
        Self {
            host: host.into(),
            user: user_from(user),
            password: pass_from(password),
        }
    }

    pub(crate) fn host(&self) -> &str {
        &self.host
    }
}

pub(crate) fn host() -> String {
    ConnectionConfig::from_environment().host
}

fn host_from(value: Option<&str>) -> String {
    value
        .unwrap_or("MiSTer address was not resolved")
        .to_string()
}

fn user_from(value: Option<&str>) -> String {
    value.unwrap_or("root").to_string()
}

fn pass_from(value: Option<&str>) -> String {
    value.unwrap_or("1").to_string()
}

pub(crate) fn connect(timeout_secs: u64) -> Result<Session> {
    connect_with(&ConnectionConfig::from_environment(), timeout_secs)
}

pub(crate) fn connect_with(config: &ConnectionConfig, timeout_secs: u64) -> Result<Session> {
    let addr = format!("{}:22", config.host)
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer host")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(timeout_secs))?;
    tcp.set_read_timeout(Some(Duration::from_secs(timeout_secs)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(timeout_secs)))?;
    let mut sess = Session::new()?;
    sess.set_tcp_stream(tcp);
    sess.handshake()?;
    sess.userauth_password(&config.user, &config.password)?;
    if !sess.authenticated() {
        return Err("SSH password authentication failed".into());
    }
    Ok(sess)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExecOutput {
    pub(crate) rc: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(crate) fn remote_subcommand(binary: &str, subcommand: &str, args: &[String]) -> String {
    let mut command = format!("{binary} {subcommand}");
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

pub(crate) fn remove_files_command(paths: &[&str]) -> String {
    format!(
        "rm -f {}",
        paths
            .iter()
            .map(|path| shell_quote(path))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

pub(crate) fn create_dir_command(path: &str) -> String {
    format!("mkdir -p {}", shell_quote(path))
}

pub(crate) fn acknowledged_main_command(command: &str) -> String {
    format!(
        "if [ -p /dev/MiSTer_cmd ] && [ -p /dev/MiSTer_cmd_reply ] && {{ pidof MiSTer_MagiKDev >/dev/null 2>&1 || pidof MiSTer_MagiK >/dev/null 2>&1; }}; then exec 8>/tmp/mister-magik/command-operation.lock; flock -w 5 8 || {{ echo 'Main command lock timed out' >&2; exit 16; }}; exec 9<>/dev/MiSTer_cmd_reply; while IFS= read -r -t 0.01 stale <&9; do :; done; heartbeat=$(sed -n 's/.*\"ts_boot_ms\":\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/main-status.json); missed=0; waits=0; printf '%s\\n' {} > /dev/MiSTer_cmd; while ! IFS= read -r -t 5 reply <&9; do waits=$((waits + 1)); if [ \"$waits\" -ge 4 ]; then echo 'Main command acknowledgement timed out' >&2; exit 17; fi; if ! pidof MiSTer_MagiKDev >/dev/null 2>&1 && ! pidof MiSTer_MagiK >/dev/null 2>&1; then echo 'Main command channel closed' >&2; exit 15; fi; next=$(sed -n 's/.*\"ts_boot_ms\":\\([0-9][0-9]*\\).*/\\1/p' /tmp/mister-magik/main-status.json); if [ -z \"$next\" ] || [ \"$next\" = \"$heartbeat\" ]; then missed=$((missed + 1)); else heartbeat=$next; missed=0; fi; if [ \"$missed\" -ge 2 ]; then echo 'Main heartbeat stopped' >&2; exit 14; fi; done; case \"$reply\" in ok|ok\\ *) printf '%s\\n' \"$reply\" ;; *) printf '%s\\n' \"$reply\" >&2; exit 13 ;; esac; else echo 'MiSTer command channel unavailable' >&2; exit 12; fi",
        shell_quote(command)
    )
}

pub(crate) fn launcher_restart_command(main_status: &str, slint_status: &str) -> String {
    format!(
        "{}; {}",
        remove_files_command(&[main_status, slint_status]),
        acknowledged_main_command("mister_magik_restart_launcher")
    )
}

pub(crate) fn exec_failure_message(context: &str, output: &ExecOutput) -> Option<String> {
    if output.rc == 0 {
        return None;
    }
    let stderr = output.stderr.trim();
    let stdout = output.stdout.trim();
    let detail = match (stderr.is_empty(), stdout.is_empty()) {
        (false, false) => format!("stderr={stderr}; stdout={stdout}"),
        (false, true) => format!("stderr={stderr}"),
        (true, false) => format!("stdout={stdout}"),
        (true, true) => "no output".to_string(),
    };
    Some(format!("{context} failed with rc={}: {detail}", output.rc))
}

#[cfg(test)]
pub(crate) fn library_sql_command_unavailable(output: &ExecOutput) -> bool {
    output.rc != 0
        && output
            .stdout
            .lines()
            .chain(output.stderr.lines())
            .any(|line| line.contains("unknown command 'library-sql'"))
}

pub(crate) fn exec(sess: &Session, command: &str, merge_stderr: bool) -> Result<ExecOutput> {
    let mut channel = sess.channel_session()?;
    if merge_stderr {
        channel.handle_extended_data(ExtendedData::Merge)?;
    }
    channel.exec(command)?;
    let mut stdout = String::new();
    channel.read_to_string(&mut stdout)?;
    let mut stderr = String::new();
    if !merge_stderr {
        channel.stderr().read_to_string(&mut stderr)?;
    }
    channel.wait_close()?;
    Ok(ExecOutput {
        rc: channel.exit_status()?,
        stdout,
        stderr,
    })
}

pub(crate) fn put(sess: &Session, local: &Path, remote: &str) -> Result<()> {
    let sftp = sess.sftp()?;
    ensure_remote_parent_dir(&sftp, Path::new(remote))?;
    let mut src = File::open(local)?;
    let mut dst = sftp.create(Path::new(remote))?;
    io::copy(&mut src, &mut dst)?;
    Ok(())
}

pub(crate) fn put_bytes(sess: &Session, remote: &str, bytes: &[u8]) -> Result<()> {
    let sftp = sess.sftp()?;
    put_bytes_with_sftp(&sftp, remote, bytes)
}

fn put_bytes_with_sftp(sftp: &ssh2::Sftp, remote: &str, bytes: &[u8]) -> Result<()> {
    ensure_remote_parent_dir(sftp, Path::new(remote))?;
    let mut dst = sftp.create(Path::new(remote))?;
    dst.write_all(bytes)?;
    Ok(())
}

fn ensure_remote_parent_dir(sftp: &ssh2::Sftp, remote: &Path) -> Result<()> {
    if let Some(parent) = remote.parent() {
        ensure_remote_dir(sftp, parent)?;
    }
    Ok(())
}

fn ensure_remote_dir(sftp: &ssh2::Sftp, remote: &Path) -> Result<()> {
    if remote.as_os_str().is_empty() || remote == Path::new("/") {
        return Ok(());
    }
    if sftp.stat(remote).is_ok() {
        return Ok(());
    }
    if let Some(parent) = remote.parent() {
        ensure_remote_dir(sftp, parent)?;
    }
    match sftp.mkdir(remote, 0o755) {
        Ok(()) => Ok(()),
        Err(_) if sftp.stat(remote).is_ok() => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn get(sess: &Session, remote: &str, local: &Path) -> Result<()> {
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent)?;
    }
    let sftp = sess.sftp()?;
    let mut src = sftp.open(Path::new(remote))?;
    let mut dst = File::create(local)?;
    io::copy(&mut src, &mut dst)?;
    Ok(())
}

pub(crate) fn tcp_probe_label_port_with(
    config: &ConnectionConfig,
    port: u16,
    timeout: Duration,
) -> String {
    let addr = match format!("{}:{port}", config.host).to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => addr,
            None => return "resolve_none".to_string(),
        },
        Err(err) => return format!("resolve_{}", err.kind() as u8),
    };

    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => "ok".to_string(),
        Err(err) => io_error_label(&err),
    }
}

fn io_error_label(err: &io::Error) -> String {
    match err.raw_os_error() {
        Some(64) => "hostdown".to_string(),
        Some(65) => "noroute".to_string(),
        Some(60) => "timeout".to_string(),
        Some(61) => "refused".to_string(),
        Some(code) => format!("os{code}"),
        None if err.kind() == io::ErrorKind::TimedOut => "timeout".to_string(),
        None if err.kind() == io::ErrorKind::ConnectionRefused => "refused".to_string(),
        None => format!("{:?}", err.kind()).to_lowercase(),
    }
}

pub(crate) fn host_wait_diagnostics_with(config: &ConnectionConfig) -> String {
    let host = config.host();
    let tcp = tcp_probe_label_port_with(config, 22, Duration::from_millis(500));
    let arp = command_summary("arp", &["-an"], Some(host));
    let ping = if cfg!(target_os = "macos") {
        command_summary("ping", &["-c", "1", "-W", "1000", host], None)
    } else {
        command_summary("ping", &["-c", "1", "-W", "1", host], None)
    };
    format!("tcp={tcp}; arp={arp}; ping={ping}")
}

fn command_summary(program: &str, args: &[&str], contains: Option<&str>) -> String {
    match Command::new(program).args(args).output() {
        Ok(output) => summarize_command_output(
            output.status.code(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
            contains,
        ),
        Err(err) => format!("error={}", err),
    }
}

fn summarize_command_output(
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
    contains: Option<&str>,
) -> String {
    let text = stdout
        .lines()
        .chain(stderr.lines())
        .filter(|line| contains.map(|needle| line.contains(needle)).unwrap_or(true))
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no matching output");
    format!("rc={} {}", code.unwrap_or(-1), text.replace('\t', " "))
}

pub(crate) fn port_open_with(config: &ConnectionConfig, timeout: Duration) -> bool {
    let Ok(mut addrs) = format!("{}:22", config.host).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_command_builders_quote_every_dynamic_value() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's here"), "'it'\"'\"'s here'");
        assert_eq!(
            remote_subcommand(
                "/media/fat/mister-magik/mister-magik-fb",
                "library-sql",
                &["SELECT *".into(), "name='Pac-Man'".into()]
            ),
            "/media/fat/mister-magik/mister-magik-fb library-sql 'SELECT *' 'name='\"'\"'Pac-Man'\"'\"''"
        );
        assert_eq!(
            remove_files_command(&["/tmp/a path", "/tmp/it's"]),
            "rm -f '/tmp/a path' '/tmp/it'\"'\"'s'"
        );
        assert_eq!(create_dir_command("/tmp/a path"), "mkdir -p '/tmp/a path'");
    }

    #[test]
    fn launcher_restart_command_keeps_fifo_safety_contract() {
        let command = launcher_restart_command("/tmp/main status", "/tmp/slint's status");
        assert!(command.starts_with("rm -f '/tmp/main status' '/tmp/slint'\"'\"'s status';"));
        assert!(command.contains("[ -p /dev/MiSTer_cmd ]"));
        assert!(command.contains("pidof MiSTer_MagiK"));
        assert!(command.contains("'mister_magik_restart_launcher'"));
        assert!(command.contains("command-operation.lock"));
        assert!(command.contains("flock -w 5 8"));
        assert!(command.contains("Main command lock timed out"));
        assert!(command.contains("exit 16"));
        assert!(command.contains("waits=$((waits + 1))"));
        assert!(command.contains("Main command acknowledgement timed out"));
        assert!(command.contains("exit 17"));
        assert!(command.contains("MiSTer_cmd_reply"));
        assert!(command.contains("exec 9<>/dev/MiSTer_cmd_reply"));
        assert!(command.contains("read -r -t 0.01 stale"));
        assert!(command.contains("exit 12"));
    }

    #[test]
    fn exec_failure_prefers_stderr_but_preserves_stdout_context() {
        let success = ExecOutput {
            rc: 0,
            stdout: "ok".into(),
            stderr: String::new(),
        };
        assert_eq!(exec_failure_message("query", &success), None);

        for (stdout, stderr, expected) in [
            ("", "", "query failed with rc=7: no output"),
            ("out", "", "query failed with rc=7: stdout=out"),
            ("", "err", "query failed with rc=7: stderr=err"),
            (
                "out",
                "err",
                "query failed with rc=7: stderr=err; stdout=out",
            ),
        ] {
            let output = ExecOutput {
                rc: 7,
                stdout: stdout.into(),
                stderr: stderr.into(),
            };
            assert_eq!(
                exec_failure_message("query", &output).as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn library_sql_fallback_requires_nonzero_unknown_command_response() {
        let mut output = ExecOutput {
            rc: 1,
            stdout: String::new(),
            stderr: "unknown command 'library-sql'".into(),
        };
        assert!(library_sql_command_unavailable(&output));
        output.rc = 0;
        assert!(!library_sql_command_unavailable(&output));
        output.rc = 1;
        output.stderr = "database is corrupt".into();
        assert!(!library_sql_command_unavailable(&output));
    }

    #[test]
    fn io_errors_map_to_stable_probe_labels_without_networking() {
        for (error, expected) in [
            (io::Error::from_raw_os_error(64), "hostdown"),
            (io::Error::from_raw_os_error(65), "noroute"),
            (io::Error::from_raw_os_error(60), "timeout"),
            (io::Error::from_raw_os_error(61), "refused"),
            (io::Error::from_raw_os_error(99), "os99"),
            (
                io::Error::new(io::ErrorKind::TimedOut, "fixture"),
                "timeout",
            ),
            (
                io::Error::new(io::ErrorKind::ConnectionRefused, "fixture"),
                "refused",
            ),
            (
                io::Error::new(io::ErrorKind::BrokenPipe, "fixture"),
                "brokenpipe",
            ),
        ] {
            assert_eq!(io_error_label(&error), expected);
        }
    }

    #[test]
    fn command_summary_handles_filters_tabs_empty_output_and_missing_status() {
        assert_eq!(
            summarize_command_output(Some(2), "skip\nmatch\tvalue\n", "err", Some("match")),
            "rc=2 match value"
        );
        assert_eq!(
            summarize_command_output(Some(0), "", "", None),
            "rc=0 no matching output"
        );
        assert_eq!(
            summarize_command_output(None, "", "failure", None),
            "rc=-1 failure"
        );
    }

    #[test]
    fn connection_environment_has_stable_defaults_and_overrides() {
        assert_eq!(host_from(None), "MiSTer address was not resolved");
        assert_eq!(user_from(None), "root");
        assert_eq!(pass_from(None), "1");
        assert_eq!(host_from(Some("192.0.2.1")), "192.0.2.1");
        assert_eq!(user_from(Some("operator")), "operator");
        assert_eq!(pass_from(Some("secret")), "secret");
        assert_eq!(
            ConnectionConfig::from_values("192.0.2.2", None, None),
            ConnectionConfig {
                host: "192.0.2.2".into(),
                user: "root".into(),
                password: "1".into(),
            }
        );
        assert_eq!(
            ConnectionConfig::from_values("192.0.2.3", Some("operator"), Some("credential"),),
            ConnectionConfig {
                host: "192.0.2.3".into(),
                user: "operator".into(),
                password: "credential".into(),
            }
        );
    }

    #[test]
    fn command_summary_reports_spawn_errors_without_panicking() {
        let summary = command_summary("/definitely/missing/mister-magik-command", &[], None);
        assert!(summary.starts_with("error="));
    }
}
