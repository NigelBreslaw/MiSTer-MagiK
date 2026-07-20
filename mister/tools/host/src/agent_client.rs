// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_agent_protocol::{self as agent_protocol, ResponseEnvelope};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::discovery::{secure_write, token_path};
use crate::remote::host;
use crate::remote::{connect, exec, put, put_bytes};
use crate::Result;

pub(crate) const AGENT_PORT: u16 = agent_protocol::PORT;
const REMOTE_AGENT: &str = "/media/fat/mister-magik-dev/mister-magik-agent";
const REMOTE_INIT: &str = "/etc/init.d/S00magik-agent";
const REMOTE_TOKEN: &str = "/media/fat/mister-magik-dev/agent.token";

#[derive(Debug)]
pub(crate) struct AgentResponse {
    pub(crate) response: Value,
    pub(crate) elapsed_ms: u128,
}

#[derive(Debug)]
pub(crate) struct AgentBinaryResponse {
    pub(crate) response: Value,
    pub(crate) payload: Vec<u8>,
    pub(crate) elapsed_ms: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VersionAction {
    Current,
    Upgrade,
    RejectNewer,
}

pub(crate) fn agent_token() -> Result<String> {
    if let Ok(token) = env::var("MISTER_AGENT_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let device_id = env::var("MISTER_DEVICE_ID")?;
    Ok(fs::read_to_string(token_path(&device_id)?)?
        .trim()
        .to_string())
}

pub(crate) fn bootstrap_agent() -> Result<()> {
    let explicit_token = env::var("MISTER_AGENT_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty());
    let device_id = env::var("MISTER_DEVICE_ID")?;
    let token_file = token_path(&device_id)?;
    let local_token = explicit_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .or_else(|| {
            fs::read_to_string(&token_file)
                .ok()
                .map(|token| token.trim().to_string())
                .filter(|token| valid_token(token))
        });
    if let Some(token) = local_token.as_ref() {
        env::set_var("MISTER_AGENT_TOKEN", token);
        if apply_installed_version_policy()? {
            return Ok(());
        }
    }

    let session = connect(3)?;
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
    env::set_var("MISTER_AGENT_TOKEN", &token);
    if apply_installed_version_policy()? {
        return Ok(());
    }

    install_agent(&session, &token)?;
    for _ in 0..20 {
        if installed_version()
            == Ok((
                agent_protocol::AGENT_VERSION,
                agent_protocol::PROTOCOL_VERSION,
            ))
        {
            cleanup_agent_backup(&session)?;
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    rollback_agent(&session)?;
    Err("MiSTer agent installation did not pass authenticated version verification".into())
}

fn apply_installed_version_policy() -> Result<bool> {
    let Ok((agent, protocol)) = installed_version() else {
        return Ok(false);
    };
    match version_action(agent, protocol) {
        VersionAction::Current => Ok(true),
        VersionAction::Upgrade => Ok(false),
        VersionAction::RejectNewer => Err(format!(
            "connected MiSTer agent is newer than this CLI (agent={agent}, protocol={protocol})"
        )
        .into()),
    }
}

fn version_action(agent: u64, protocol: u64) -> VersionAction {
    if agent == agent_protocol::AGENT_VERSION && protocol == agent_protocol::PROTOCOL_VERSION {
        VersionAction::Current
    } else if agent > agent_protocol::AGENT_VERSION || protocol > agent_protocol::PROTOCOL_VERSION {
        VersionAction::RejectNewer
    } else {
        VersionAction::Upgrade
    }
}

fn installed_version() -> std::result::Result<(u64, u64), String> {
    let reply = agent_request("ping", json!({}), Duration::from_millis(500))
        .map_err(|error| error.to_string())?;
    let result = reply.response.get("result").unwrap_or(&Value::Null);
    Ok((
        result
            .get("agent_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| "missing agent version".to_string())?,
        result
            .get("protocol_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| "missing protocol version".to_string())?,
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
    let output = Command::new("scripts/build-mister-agent.sh").output()?;
    if !output.status.success() {
        return Err(format!(
            "could not build current MiSTer agent: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let path = String::from_utf8(output.stdout)?
        .lines()
        .last()
        .ok_or("agent build did not report its artifact")?
        .to_string();
    let path = PathBuf::from(path);
    if !path.is_file() {
        return Err("agent build artifact is missing".into());
    }
    Ok(path)
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
    exec(session, "mkdir -p /media/fat/mister-magik-dev", true)?;
    put(session, Path::new(&binary), &format!("{REMOTE_AGENT}.new"))?;
    let staged_init = "/media/fat/mister-magik-dev/S00magik-agent.new";
    put_bytes(session, staged_init, init)?;
    put_bytes(
        session,
        &format!("{REMOTE_TOKEN}.new"),
        format!("{token}\n").as_bytes(),
    )?;
    cleanup_agent_backup(session)?;
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

fn rollback_agent(session: &ssh2::Session) -> Result<()> {
    let command = format!(
        "mount -o remount,rw /; {init} stop 2>/dev/null || true; if [ -f {agent}.prev ]; then mv {agent}.prev {agent}; elif [ -f {agent}.prev-missing ]; then rm -f {agent}; fi; if [ -f {init}.prev ]; then mv {init}.prev {init}; elif [ -f {init}.prev-missing ]; then rm -f {init}; fi; if [ -f {token}.prev ]; then mv {token}.prev {token}; elif [ -f {token}.prev-missing ]; then rm -f {token}; fi; rm -f {agent}.prev-missing {init}.prev-missing {token}.prev-missing; {init} start 2>/dev/null || true; sync; mount -o remount,ro / || true",
        agent = REMOTE_AGENT,
        init = REMOTE_INIT,
        token = REMOTE_TOKEN,
    );
    exec(session, &command, true)?;
    Ok(())
}

fn cleanup_agent_backup(session: &ssh2::Session) -> Result<()> {
    let command = format!(
        "mount -o remount,rw /; rm -f {agent}.prev {init}.prev {token}.prev {agent}.prev-missing {init}.prev-missing {token}.prev-missing; sync; mount -o remount,ro / || true",
        agent = REMOTE_AGENT,
        init = REMOTE_INIT,
        token = REMOTE_TOKEN,
    );
    exec(session, &command, true)?;
    Ok(())
}

pub(crate) fn agent_request(cmd: &str, args: Value, timeout: Duration) -> Result<AgentResponse> {
    let token = agent_token()?;
    let addr = format!("{}:{AGENT_PORT}", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(&token, 1, cmd, args);
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
    let token = agent_token()?;
    let addr = format!("{}:{AGENT_PORT}", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(&token, 1, cmd, args);
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
                agent_request("ping", json!({}), connect_timeout)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(crate) fn agent_binary_request_bounded(
    cmd: &str,
    args: Value,
    timeout: Duration,
    max_payload_bytes: u64,
) -> Result<AgentBinaryResponse> {
    let token = agent_token()?;
    let addr = format!("{}:{AGENT_PORT}", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(&token, 1, cmd, args);
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    read_agent_binary_response(&mut reader, start, max_payload_bytes)
}

fn read_agent_binary_response<R: BufRead>(
    reader: &mut R,
    start: Instant,
    max_payload_bytes: u64,
) -> Result<AgentBinaryResponse> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response = parse_agent_response_line(line, start)?.response;
    let payload_bytes = agent_binary_payload_len(&response)?;
    if u64::try_from(payload_bytes)? > max_payload_bytes {
        return Err(format!(
            "agent binary payload too large: {payload_bytes} bytes (max {max_payload_bytes})"
        )
        .into());
    }
    let mut payload = Vec::new();
    payload.try_reserve_exact(payload_bytes)?;
    payload.resize(payload_bytes, 0);
    reader.read_exact(&mut payload)?;
    Ok(AgentBinaryResponse {
        response,
        payload,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

pub(crate) fn agent_stream_request_reader(
    cmd: &str,
    args: Value,
    payload: &mut dyn Read,
    timeout: Duration,
) -> Result<AgentResponse> {
    let token = agent_token()?;
    let addr = format!("{}:{AGENT_PORT}", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer agent host")?;
    let request = agent_protocol::request(&token, 1, cmd, args);
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    if let Err(write_error) = io::copy(payload, &mut stream).and_then(|_| stream.flush()) {
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

fn agent_binary_payload_len(response: &Value) -> Result<usize> {
    agent_protocol::binary_payload_len(response).map_err(|error| {
        error
            .replace("missing payload", "missing result payload")
            .into()
    })
}

pub(crate) fn verify_agent_deploy_result(
    result: &Value,
    expected_bytes: u64,
    expected_remote: &str,
    expected_checksum: &str,
) -> Result<u64> {
    let remote = result.get("remote").and_then(Value::as_str).unwrap_or("");
    if remote != expected_remote {
        return Err(format!(
            "agent deploy remote mismatch expected={expected_remote} actual={remote}"
        )
        .into());
    }
    let remote_bytes = result
        .get("remote_bytes")
        .and_then(Value::as_u64)
        .ok_or("agent deploy response missing remote_bytes")?;
    if remote_bytes != expected_bytes {
        return Err(format!(
            "agent deployed size mismatch expected={expected_bytes} remote={remote_bytes}"
        )
        .into());
    }
    if result.get("checksum_algorithm").and_then(Value::as_str) != Some("sha256")
        || result.get("checksum").and_then(Value::as_str) != Some(expected_checksum)
        || result.get("published").and_then(Value::as_bool) != Some(true)
        || result.get("rolled_back").and_then(Value::as_bool) != Some(false)
    {
        return Err("agent deployment was not verified and authoritatively published".into());
    }
    Ok(remote_bytes)
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

        assert!(parse_agent_response_line(String::new(), start)
            .expect_err("empty response should fail")
            .to_string()
            .contains("empty response"));
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
    fn agent_binary_payload_len_prefers_payload_bytes_and_allows_raw_fallback() {
        assert_eq!(
            agent_binary_payload_len(&json!({
                "result": {
                    "payload_bytes": 7,
                    "raw_bytes": 99,
                }
            }))
            .expect("payload len"),
            7
        );
        assert_eq!(
            agent_binary_payload_len(&json!({
                "result": {
                    "raw_bytes": 11,
                }
            }))
            .expect("raw fallback len"),
            11
        );
    }

    #[test]
    fn in_memory_binary_response_is_bounded_and_requires_exact_payload() {
        let mut success =
            Cursor::new(b"{\"ok\":true,\"result\":{\"payload_bytes\":4}}\nDATA".to_vec());
        let response = read_agent_binary_response(&mut success, Instant::now(), 4).unwrap();
        assert_eq!(response.payload, b"DATA");

        let mut oversized =
            Cursor::new(b"{\"ok\":true,\"result\":{\"payload_bytes\":5}}\nDATA!".to_vec());
        assert!(
            read_agent_binary_response(&mut oversized, Instant::now(), 4)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );

        let mut truncated =
            Cursor::new(b"{\"ok\":true,\"result\":{\"payload_bytes\":5}}\nDATA".to_vec());
        let error = read_agent_binary_response(&mut truncated, Instant::now(), 5).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn in_memory_binary_response_surfaces_agent_error_before_payload_read() {
        let mut reader = Cursor::new(b"{\"ok\":false,\"error\":\"denied\"}\nignored".to_vec());

        assert_eq!(
            read_agent_binary_response(&mut reader, Instant::now(), 10)
                .unwrap_err()
                .to_string(),
            "denied"
        );
    }

    #[test]
    fn agent_binary_payload_len_rejects_missing_or_oversized_values() {
        assert!(agent_binary_payload_len(&json!({"result": {}}))
            .expect_err("missing payload len should fail")
            .to_string()
            .contains("missing result payload byte count"));
        assert!(agent_binary_payload_len(&json!({
            "result": {
                "payload_bytes": agent_protocol::MAX_BINARY_PAYLOAD_BYTES + 1,
            }
        }))
        .expect_err("oversized payload len should fail")
        .to_string()
        .contains("payload too large"));
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
                agent_protocol::PROTOCOL_VERSION
            ),
            VersionAction::Current
        );
        assert_eq!(version_action(0, 0), VersionAction::Upgrade);
        assert_eq!(
            version_action(agent_protocol::AGENT_VERSION + 1, 0),
            VersionAction::RejectNewer
        );
        assert_eq!(
            version_action(0, agent_protocol::PROTOCOL_VERSION + 1),
            VersionAction::RejectNewer
        );
    }

    #[test]
    fn managed_tokens_require_256_bits_of_hex() {
        assert!(valid_token(&"ab".repeat(32)));
        assert!(!valid_token("short"));
        assert!(!valid_token(&"zz".repeat(32)));
    }
}
