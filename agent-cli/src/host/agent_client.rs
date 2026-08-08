// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_agent_protocol::{self as agent_protocol, ResponseEnvelope};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::Result;
use super::discovery::{secure_write, token_path};
use super::remote::{ConnectionConfig, connect_with, exec, host, put, put_bytes};

pub(crate) const AGENT_PORT: u16 = agent_protocol::PORT;
const REMOTE_AGENT: &str = "/media/fat/mister-magik-dev/mister-magik-agent";
const REMOTE_INIT: &str = "/etc/init.d/S00magik-agent";
const REMOTE_TOKEN: &str = "/media/fat/mister-magik-dev/agent.token";

#[derive(Debug)]
pub(crate) struct AgentResponse {
    pub(crate) response: Value,
    pub(crate) elapsed_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentEndpoint {
    host: String,
    token: String,
}

impl AgentEndpoint {
    pub(crate) fn new(host: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            token: token.into(),
        }
    }

    pub(crate) fn from_environment() -> Result<Self> {
        Ok(Self::new(host(), agent_token()?))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VersionAction {
    Current,
    Upgrade,
    RejectNewer,
}

pub(crate) fn agent_token() -> Result<String> {
    let device_id = env::var("MISTER_DEVICE_ID")?;
    agent_token_for_device(&device_id, env::var("MISTER_AGENT_TOKEN").ok().as_deref())
}

pub(crate) fn agent_token_for_device(
    device_id: &str,
    explicit_token: Option<&str>,
) -> Result<String> {
    if let Some(token) = explicit_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return Ok(token.to_string());
    }
    Ok(fs::read_to_string(token_path(device_id)?)?
        .trim()
        .to_string())
}

pub(crate) fn bootstrap_agent_with(
    connection: &ConnectionConfig,
    device_id: &str,
    explicit_token: Option<&str>,
) -> Result<String> {
    let token_file = token_path(device_id)?;
    let stored_token = fs::read_to_string(&token_file).ok();
    let local_token = preferred_token(explicit_token, stored_token.as_deref());
    if let Some(token) = local_token.as_ref() {
        let endpoint = AgentEndpoint::new(connection.host(), token);
        if apply_installed_version_policy(&endpoint)? {
            return Ok(token.clone());
        }
    }

    let session = connect_with(connection, 3)?;
    let token = if explicit_token.is_some() {
        local_token.ok_or("MISTER_AGENT_TOKEN is empty")?
    } else {
        let remote = exec(
            &session,
            "cat /media/fat/mister-magik-dev/agent.token 2>/dev/null || true",
            true,
        )?
        .stdout
        .trim()
        .to_string();
        let token = if valid_token(&remote) {
            remote
        } else {
            local_token.unwrap_or(generate_token()?)
        };
        secure_write(&token_file, format!("{token}\n").as_bytes())?;
        token
    };
    let endpoint = AgentEndpoint::new(connection.host(), &token);
    if apply_installed_version_policy(&endpoint)? {
        return Ok(token);
    }

    install_agent(&session, &token)?;
    for _ in 0..20 {
        if installed_identity(&endpoint)
            == Ok((
                agent_protocol::AGENT_VERSION,
                agent_protocol::PROTOCOL_VERSION,
                true,
                true,
                true,
            ))
        {
            cleanup_agent_backup(&session)?;
            return Ok(token);
        }
        thread::sleep(Duration::from_millis(100));
    }
    rollback_agent(&session)?;
    Err("MiSTer agent installation did not pass authenticated version verification".into())
}

fn preferred_token(explicit: Option<&str>, stored: Option<&str>) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .or_else(|| {
            stored
                .map(str::trim)
                .filter(|token| valid_token(token))
                .map(str::to_string)
        })
}

fn apply_installed_version_policy(endpoint: &AgentEndpoint) -> Result<bool> {
    let Ok((agent, protocol, has_capture_v2, has_device_telemetry_v2, has_launcher_automation)) =
        installed_identity(endpoint)
    else {
        return Ok(false);
    };
    match version_action(
        agent,
        protocol,
        has_capture_v2,
        has_device_telemetry_v2,
        has_launcher_automation,
    ) {
        VersionAction::Current => Ok(true),
        VersionAction::Upgrade => Ok(false),
        VersionAction::RejectNewer => Err(format!(
            "connected MiSTer agent is newer than this CLI (agent={agent}, protocol={protocol})"
        )
        .into()),
    }
}

fn version_action(
    agent: u64,
    protocol: u64,
    has_capture_v2: bool,
    has_device_telemetry_v2: bool,
    has_launcher_automation: bool,
) -> VersionAction {
    if agent == agent_protocol::AGENT_VERSION
        && protocol == agent_protocol::PROTOCOL_VERSION
        && has_capture_v2
        && has_device_telemetry_v2
        && has_launcher_automation
    {
        VersionAction::Current
    } else if agent > agent_protocol::AGENT_VERSION || protocol > agent_protocol::PROTOCOL_VERSION {
        VersionAction::RejectNewer
    } else {
        VersionAction::Upgrade
    }
}

fn installed_identity(
    endpoint: &AgentEndpoint,
) -> std::result::Result<(u64, u64, bool, bool, bool), String> {
    let reply = agent_request_at(endpoint, "ping", json!({}), Duration::from_millis(500))
        .map_err(|error| error.to_string())?;
    let result = reply.response.get("result").unwrap_or(&Value::Null);
    let agent = result
        .get("agent_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "missing agent version".to_string())?;
    let protocol = result
        .get("protocol_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "missing protocol version".to_string())?;
    let has_capture_v2 = result
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities.iter().any(|capability| {
                capability.as_str() == Some(agent_protocol::FRAMEBUFFER_CAPTURE_CAPABILITY)
            })
        });
    let has_launcher_automation = result
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities.iter().any(|capability| {
                capability.as_str() == Some(agent_protocol::LAUNCHER_AUTOMATION_CAPABILITY)
            })
        });
    let has_device_telemetry_v2 = result
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|capabilities| {
            capabilities.iter().any(|capability| {
                capability.as_str() == Some(agent_protocol::DEVICE_TELEMETRY_CAPABILITY)
            })
        });
    Ok((
        agent,
        protocol,
        has_capture_v2,
        has_device_telemetry_v2,
        has_launcher_automation,
    ))
}

fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn generate_token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn build_agent() -> Result<PathBuf> {
    const TARGET: &str = "armv7-unknown-linux-gnueabihf";
    let repository = env::current_dir()?;
    ensure_protocol_source_matches_cli(&repository)?;
    let path = repository
        .join("mister/tools/agent/target")
        .join(TARGET)
        .join("release/mister-magik-agent");
    let explicit_cross = env::var("MISTER_ARM_BUILD_BACKEND").as_deref() == Ok("cross");
    if explicit_cross {
        let mut command = Command::new("cross");
        command
            .current_dir(repository.join("mister/tools/agent"))
            .env("RUSTC_WRAPPER", "")
            .env("RUSTFLAGS", "-D warnings -C target-cpu=cortex-a9")
            .args(["build", "--target", TARGET, "--release", "--locked"]);
        run_agent_build_bounded(&mut command)?;
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is unavailable")?;
        let target_dir = PathBuf::from("/private/tmp/mister-magik-agent-apple-container-target");
        fs::create_dir_all(&target_dir)?;
        let cpus = thread::available_parallelism()?.get().to_string();
        let image = "mister-magik-cross-armv7:ubuntu20-arm64";
        let mut probe = Command::new("container");
        probe.args(["run", "--arch", "arm64", "--rm", image, "true"]);
        if !probe.status().is_ok_and(|status| status.success()) {
            let mut build = Command::new("container");
            build.current_dir(repository.join("apps/mister")).args([
                "build",
                "--arch",
                "arm64",
                "--file",
                "Dockerfile.cross-armv7",
                "--tag",
                image,
                ".",
            ]);
            run_agent_build_bounded(&mut build)?;
        }
        let mut command = Command::new("container");
        command
            .args(["run", "--arch", "arm64", "--rm", "--cpus"])
            .arg(&cpus)
            .args(["--memory", "8g", "--env", "CARGO_HOME=/cargo", "--env", "CARGO_TARGET_DIR=/target"])
            .arg("--env")
            .arg(format!("CARGO_BUILD_JOBS={cpus}"))
            .args(["--env", "RUSTC_WRAPPER=", "--env", "RUSTFLAGS=-D warnings -C target-cpu=cortex-a9"])
            .arg("--volume")
            .arg(format!("{}:/cargo", home.join(".cargo").display()))
            .arg("--volume")
            .arg(format!(
                "{}:/rust:ro",
                home.join(".rustup/toolchains/stable-aarch64-unknown-linux-gnu")
                    .display()
            ))
            .arg("--volume")
            .arg(format!("{}:/project", repository.display()))
            .arg("--volume")
            .arg(format!("{}:/target", target_dir.display()))
            .args([
                "--workdir",
                "/project/mister/tools/agent",
                image,
                "sh",
                "-lc",
                "PATH=/rust/bin:$PATH cargo build --target armv7-unknown-linux-gnueabihf --release --locked",
            ]);
        run_agent_build_bounded(&mut command)?;
        let built = target_dir.join(TARGET).join("release/mister-magik-agent");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(built, &path)?;
    } else {
        return Err("agent build requires Apple container locally; cross is reserved for explicit CI/operator comparison".into());
    }
    if !path.is_file() {
        return Err("agent build artifact is missing".into());
    }
    Ok(path)
}

fn ensure_protocol_source_matches_cli(repository: &Path) -> Result<()> {
    let source = fs::read_to_string(repository.join("crates/agent-protocol/src/lib.rs"))?;
    validate_protocol_source_identity(&source)
}

fn validate_protocol_source_identity(source: &str) -> Result<()> {
    let source_agent = rust_u64_constant(source, "AGENT_VERSION")?;
    let source_protocol = rust_u64_constant(source, "PROTOCOL_VERSION")?;
    let cli_identity = (
        agent_protocol::AGENT_VERSION,
        agent_protocol::PROTOCOL_VERSION,
    );
    let source_identity = (source_agent, source_protocol);
    if source_identity != cli_identity {
        return Err(format!(
            "mister CLI is stale relative to agent protocol source (cli={}.{}, source={}.{}); commit and push the protocol change so pre-push assurance rebuilds the host tool",
            cli_identity.0, cli_identity.1, source_identity.0, source_identity.1
        )
        .into());
    }
    Ok(())
}

fn rust_u64_constant(source: &str, name: &str) -> Result<u64> {
    let prefix = format!("pub const {name}: u64 = ");
    source
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .and_then(|value| value.strip_suffix(';'))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("agent protocol source is missing {name}").into())
}

fn run_agent_build_bounded(command: &mut Command) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    let started = Instant::now();
    let deadline = Duration::from_secs(30 * 60);
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => return Err(format!("agent build failed with {status}").into()),
            None if started.elapsed() < deadline => thread::sleep(Duration::from_millis(100)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("agent build exceeded its 1800s deadline".into());
            }
        }
    }
}

fn install_agent(session: &ssh2::Session, token: &str) -> Result<()> {
    let binary = build_agent()?;
    let init = br#"#!/bin/sh
stop_agent() {
  pids="$(pidof mister-magik-agent 2>/dev/null || true)"
  [ -z "$pids" ] && return 0
  kill $pids 2>/dev/null || true
  i=0
  while [ "$i" -lt 20 ]; do
    pidof mister-magik-agent >/dev/null 2>&1 || return 0
    sleep 0.1
    i=$((i + 1))
  done
  kill -9 $(pidof mister-magik-agent 2>/dev/null || true) 2>/dev/null || true
}
case "$1" in
  start) /media/fat/mister-magik-dev/mister-magik-agent net-boot >/tmp/mister-magik-agent.boot.out 2>&1 & ;;
  stop) stop_agent ;;
  restart) stop_agent; /media/fat/mister-magik-dev/mister-magik-agent net-boot >/tmp/mister-magik-agent.boot.out 2>&1 & ;;
  *) exit 2 ;;
esac
"#;
    reconcile_interrupted_agent_transaction(session)?;
    exec(session, "mkdir -p /media/fat/mister-magik-dev", true)?;
    put(session, Path::new(&binary), &format!("{REMOTE_AGENT}.new"))?;
    let staged_init = "/media/fat/mister-magik-dev/S00magik-agent.new";
    put_bytes(session, staged_init, init)?;
    put_bytes(
        session,
        &format!("{REMOTE_TOKEN}.new"),
        format!("{token}\n").as_bytes(),
    )?;
    let command = format!(
        "set -eu; mount -o remount,rw /; {init} stop 2>/dev/null || true; if [ -f {agent} ]; then cp -p {agent} {agent}.prev; else : > {agent}.prev-missing; fi; if [ -f {init} ]; then cp -p {init} {init}.prev; else : > {init}.prev-missing; fi; if [ -f {token} ]; then cp -p {token} {token}.prev; else : > {token}.prev-missing; fi; mv {agent}.new {agent}; mv {staged_init} {init}; mv {token}.new {token}; chmod 755 {agent} {init}; chmod 600 {token}; {init} start; sync; mount -o remount,ro / || true",
        agent = REMOTE_AGENT,
        init = REMOTE_INIT,
        token = REMOTE_TOKEN,
        staged_init = staged_init,
    );
    let output = exec(session, &command, true)?;
    if output.rc != 0 {
        rollback_agent(session)?;
        return Err("MiSTer agent activation failed".into());
    }
    Ok(())
}

fn reconcile_interrupted_agent_transaction(session: &ssh2::Session) -> Result<()> {
    let output = exec(session, &interrupted_agent_transaction_command(), false)?;
    if output.rc != 0 {
        return Err("cannot inspect the prior MiSTer agent transaction".into());
    }
    if output.stdout.trim() == "interrupted" {
        rollback_agent(session)?;
    }
    Ok(())
}

fn interrupted_agent_transaction_command() -> String {
    let artifacts = [REMOTE_AGENT, REMOTE_INIT, REMOTE_TOKEN]
        .into_iter()
        .flat_map(|path| [format!("{path}.prev"), format!("{path}.prev-missing")])
        .collect::<Vec<_>>();
    format!(
        "if {}; then echo interrupted; else echo clean; fi",
        artifacts
            .iter()
            .map(|path| format!("test -e {path}"))
            .collect::<Vec<_>>()
            .join(" || ")
    )
}

fn rollback_agent(session: &ssh2::Session) -> Result<()> {
    exec(session, &rollback_agent_command(), true)?;
    Ok(())
}

fn rollback_agent_command() -> String {
    format!(
        "mount -o remount,rw /; {init} stop 2>/dev/null || true; if [ -f {agent}.prev ]; then mv {agent}.prev {agent}; elif [ -f {agent}.prev-missing ]; then rm -f {agent}; fi; if [ -f {init}.prev ]; then mv {init}.prev {init}; elif [ -f {init}.prev-missing ]; then rm -f {init}; fi; if [ -f {token}.prev ]; then mv {token}.prev {token}; elif [ -f {token}.prev-missing ]; then rm -f {token}; fi; rm -f {agent}.prev-missing {init}.prev-missing {token}.prev-missing; {init} start 2>/dev/null || true; sync; mount -o remount,ro / || true",
        agent = REMOTE_AGENT,
        init = REMOTE_INIT,
        token = REMOTE_TOKEN,
    )
}

fn cleanup_agent_backup(session: &ssh2::Session) -> Result<()> {
    exec(session, &cleanup_agent_backup_command(), true)?;
    Ok(())
}

fn cleanup_agent_backup_command() -> String {
    format!(
        "mount -o remount,rw /; rm -f {agent}.prev {init}.prev {token}.prev {agent}.prev-missing {init}.prev-missing {token}.prev-missing; sync; mount -o remount,ro / || true",
        agent = REMOTE_AGENT,
        init = REMOTE_INIT,
        token = REMOTE_TOKEN,
    )
}

pub(crate) fn agent_request(cmd: &str, args: Value, timeout: Duration) -> Result<AgentResponse> {
    agent_request_at(&AgentEndpoint::from_environment()?, cmd, args, timeout)
}

pub(crate) fn agent_request_at(
    endpoint: &AgentEndpoint,
    cmd: &str,
    args: Value,
    timeout: Duration,
) -> Result<AgentResponse> {
    let addr = format!("{}:{AGENT_PORT}", endpoint.host)
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(&endpoint.token, 1, cmd, args);
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    parse_agent_response_line(line, start)
}

pub(crate) fn agent_request_with_liveness(
    cmd: &str,
    args: Value,
    connect_timeout: Duration,
) -> Result<AgentResponse> {
    let endpoint = AgentEndpoint::from_environment()?;
    let addr = format!("{}:{AGENT_PORT}", endpoint.host)
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(&endpoint.token, 1, cmd, args);
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, connect_timeout)?;
    stream.set_write_timeout(Some(connect_timeout))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;
    stream.set_read_timeout(Some(Duration::from_secs(6)))?;
    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    loop {
        match reader.read_line(&mut line) {
            Ok(0) => return Err("MiSTer agent connection closed".into()),
            Ok(_) => return parse_agent_response_line(line, start),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                // Renew the bounded transport wait only when a separate agent
                // request proves that the remote service is still responsive.
                agent_request_at(&endpoint, "ping", json!({}), connect_timeout)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(crate) fn agent_telemetry_until_screensaver_profile_complete(
    endpoint: &AgentEndpoint,
    timeout: Duration,
) -> Result<Vec<Value>> {
    let addr = format!("{}:{AGENT_PORT}", endpoint.host)
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(
        &endpoint.token,
        1,
        "device_telemetry_stream_v2",
        json!({"analytics_mode": "process", "cadence_ms": 250}),
    );
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    parse_agent_response_line(line, started)?;

    let mut samples = Vec::new();
    let mut legacy_complete_status_sequence = None;
    while started.elapsed() < timeout {
        if super::attended_operation_interrupted() {
            return Err("screensaver benchmark interrupted".into());
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err("MiSTer telemetry stream closed before profile completion".into()),
            Ok(_) => {
                let sample = parse_device_telemetry_sample(&line)?;
                let state = sample
                    .pointer("/launcher/screensaver_profile_state")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let status_sequence = sample
                    .pointer("/launcher/status_sequence")
                    .and_then(Value::as_u64);
                let written_sequence = sample
                    .pointer("/launcher/status_written_sequence")
                    .and_then(Value::as_u64);
                samples.push(sample);
                match state.as_deref() {
                    Some("complete") => {
                        if status_sequence.is_some() && status_sequence == written_sequence {
                            return Ok(samples);
                        }
                        if written_sequence.is_none() {
                            if let Some(first_complete) = legacy_complete_status_sequence {
                                if status_sequence.is_some_and(|sequence| sequence > first_complete)
                                {
                                    return Ok(samples);
                                }
                            } else if let Some(sequence) = status_sequence {
                                legacy_complete_status_sequence = Some(sequence);
                            }
                        }
                    }
                    Some("failed") => {
                        return Err(
                            "installed launcher reported screensaver profile failure".into()
                        );
                    }
                    _ => {}
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("screensaver profile did not complete within the bounded timeout".into())
}

pub(crate) fn agent_telemetry_for_duration(
    endpoint: &AgentEndpoint,
    duration: Duration,
) -> Result<Vec<Value>> {
    agent_telemetry_for_duration_at_cadence(endpoint, duration, 250)
}

pub(crate) fn agent_telemetry_for_duration_at_cadence(
    endpoint: &AgentEndpoint,
    duration: Duration,
    cadence_ms: u64,
) -> Result<Vec<Value>> {
    let addr = format!("{}:{AGENT_PORT}", endpoint.host)
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(
        &endpoint.token,
        1,
        "device_telemetry_stream_v2",
        json!({"analytics_mode": "process", "cadence_ms": cadence_ms}),
    );
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    parse_agent_response_line(line, started)?;

    let mut samples = Vec::new();
    while started.elapsed() < duration {
        if super::attended_operation_interrupted() {
            return Err("device telemetry collection interrupted".into());
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err("MiSTer telemetry stream closed during bounded collection".into()),
            Ok(_) => samples.push(parse_device_telemetry_sample(&line)?),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    if samples.is_empty() {
        return Err("bounded device telemetry collection produced no samples".into());
    }
    Ok(samples)
}

fn parse_device_telemetry_sample(line: &str) -> Result<Value> {
    let sample: Value = serde_json::from_str(line.trim())?;
    if sample.get("schema").and_then(Value::as_str) != Some("mister-magik-device-telemetry-v2")
        || sample
            .pointer("/presentation/schema")
            .and_then(Value::as_str)
            != Some("mister-magik-presentation-telemetry-snapshot-v1")
        || sample
            .pointer("/presentation/source")
            .and_then(Value::as_str)
            != Some("fpga-owned-vblank-telemetry")
    {
        return Err("MiSTer agent returned non-authoritative device telemetry".into());
    }
    Ok(sample)
}

#[cfg(test)]
fn write_agent_stream_payload<T: Read + Write>(
    mut stream: T,
    payload: &[u8],
    start: Instant,
) -> Result<AgentResponse> {
    if let Err(write_error) = stream.write_all(payload).and_then(|_| stream.flush()) {
        let mut line = String::new();
        match BufReader::new(stream).read_line(&mut line) {
            Ok(0) | Err(_) => return Err(write_error.into()),
            Ok(_) => return parse_agent_response_line(line, start),
        }
    }
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    parse_agent_response_line(line, start)
}

fn parse_agent_response_line(line: String, start: Instant) -> Result<AgentResponse> {
    match agent_protocol::parse_response_line(&line)
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?
    {
        ResponseEnvelope::Ok { full, .. } => Ok(AgentResponse {
            response: full,
            elapsed_ms: start.elapsed().as_millis(),
        }),
        ResponseEnvelope::Error(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Cursor, Error, ErrorKind};

    struct RejectingStream {
        response: Cursor<Vec<u8>>,
    }

    impl Read for RejectingStream {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.response.read(buffer)
        }
    }

    impl Write for RejectingStream {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(Error::new(ErrorKind::BrokenPipe, "broken pipe"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn agent_response_parser_rejects_empty_and_error_responses() {
        let start = Instant::now();

        assert!(
            parse_agent_response_line(String::new(), start)
                .expect_err("empty response should fail")
                .to_string()
                .contains("empty response")
        );
        assert_eq!(
            parse_agent_response_line(
                "{\"ok\":false,\"error\":\"permission denied\"}\n".to_string(),
                start
            )
            .expect_err("agent error should fail")
            .to_string(),
            "permission denied"
        );
    }

    #[test]
    fn agent_response_parser_accepts_successful_json() {
        let response = parse_agent_response_line(
            "{\"ok\":true,\"result\":{\"value\":42}}\n".to_string(),
            Instant::now(),
        )
        .expect("success response");

        assert_eq!(response.response["result"]["value"], 42);
    }

    #[test]
    fn stream_request_surfaces_agent_rejection_instead_of_broken_pipe() {
        let stream = RejectingStream {
            response: Cursor::new(
                serde_json::to_vec(&json!({
                    "id": 1,
                    "ok": false,
                    "error": "deploy remote must be under /media/fat/mister-magik"
                }))
                .expect("encode rejection"),
            ),
        };
        let payload = [0u8; 1];
        let error = write_agent_stream_payload(stream, &payload, Instant::now())
            .expect_err("agent rejection should fail")
            .to_string();

        assert_eq!(error, "deploy remote must be under /media/fat/mister-magik");
    }

    #[test]
    fn version_policy_accepts_exact_upgrades_old_and_rejects_newer() {
        assert_eq!(
            version_action(
                agent_protocol::AGENT_VERSION,
                agent_protocol::PROTOCOL_VERSION,
                true,
                true,
                true,
            ),
            VersionAction::Current
        );
        assert_eq!(
            version_action(0, 0, false, false, false),
            VersionAction::Upgrade
        );
        assert_eq!(
            version_action(
                agent_protocol::AGENT_VERSION,
                agent_protocol::PROTOCOL_VERSION,
                false,
                true,
                true,
            ),
            VersionAction::Upgrade
        );
        assert_eq!(
            version_action(agent_protocol::AGENT_VERSION + 1, 0, false, false, false,),
            VersionAction::RejectNewer
        );
        assert_eq!(
            version_action(0, agent_protocol::PROTOCOL_VERSION + 1, false, false, false,),
            VersionAction::RejectNewer
        );
    }

    #[test]
    fn protocol_source_identity_parser_rejects_stale_cli_inputs() {
        let current = format!(
            "pub const AGENT_VERSION: u64 = {};\npub const PROTOCOL_VERSION: u64 = {};\n",
            agent_protocol::AGENT_VERSION,
            agent_protocol::PROTOCOL_VERSION
        );
        assert_eq!(
            rust_u64_constant(&current, "AGENT_VERSION").unwrap(),
            agent_protocol::AGENT_VERSION
        );
        assert_eq!(
            rust_u64_constant(&current, "PROTOCOL_VERSION").unwrap(),
            agent_protocol::PROTOCOL_VERSION
        );
        validate_protocol_source_identity(&current).unwrap();
        let stale = format!(
            "pub const AGENT_VERSION: u64 = {};\npub const PROTOCOL_VERSION: u64 = {};\n",
            agent_protocol::AGENT_VERSION,
            agent_protocol::PROTOCOL_VERSION + 1
        );
        assert!(
            validate_protocol_source_identity(&stale)
                .unwrap_err()
                .to_string()
                .contains("mister CLI is stale")
        );
    }

    #[test]
    fn managed_tokens_require_256_bits_of_hex() {
        assert!(valid_token(&"ab".repeat(32)));
        assert!(!valid_token("short"));
        assert!(!valid_token(&"zz".repeat(32)));
    }

    #[test]
    fn token_preference_uses_nonempty_explicit_then_valid_stored_token() {
        let stored = "b".repeat(64);
        assert_eq!(
            preferred_token(Some(" explicit "), Some(&stored)).as_deref(),
            Some("explicit")
        );
        assert_eq!(
            preferred_token(Some("  "), Some(&stored)).as_deref(),
            Some(stored.as_str())
        );
        assert_eq!(preferred_token(None, Some("invalid")), None);
    }

    #[test]
    fn agent_endpoint_retains_forwarded_host_and_token() {
        let endpoint = AgentEndpoint::new("192.0.2.4", "token-value");

        assert_eq!(endpoint.host, "192.0.2.4");
        assert_eq!(endpoint.token, "token-value");
    }

    #[test]
    fn generated_tokens_are_valid_and_not_constant() {
        let first = generate_token().unwrap();
        let second = generate_token().unwrap();
        assert!(valid_token(&first));
        assert!(valid_token(&second));
        assert_ne!(first, second);
    }

    #[test]
    fn rollback_and_cleanup_commands_cover_every_transaction_artifact() {
        let rollback = rollback_agent_command();
        let cleanup = cleanup_agent_backup_command();
        let interrupted = interrupted_agent_transaction_command();
        for path in [REMOTE_AGENT, REMOTE_INIT, REMOTE_TOKEN] {
            assert!(rollback.contains(path));
            assert!(rollback.contains(&format!("{path}.prev")));
            assert!(rollback.contains(&format!("{path}.prev-missing")));
            assert!(cleanup.contains(&format!("{path}.prev")));
            assert!(cleanup.contains(&format!("{path}.prev-missing")));
            assert!(interrupted.contains(&format!("{path}.prev")));
            assert!(interrupted.contains(&format!("{path}.prev-missing")));
        }
        assert!(rollback.contains("mount -o remount,ro /"));
        assert!(cleanup.contains("mount -o remount,ro /"));
    }
}
