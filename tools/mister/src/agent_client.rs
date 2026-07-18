// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_agent_protocol::{self as agent_protocol, ResponseEnvelope};
use serde_json::Value;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::remote::host;
use crate::Result;

pub(crate) const AGENT_PORT: u16 = agent_protocol::PORT;
const AGENT_TOKEN_LOCAL: &str = "build/mister-agent.token";

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

pub(crate) fn agent_token() -> Result<String> {
    if let Ok(token) = env::var("MISTER_AGENT_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    match fs::read_to_string(AGENT_TOKEN_LOCAL) {
        Ok(token) => Ok(token.trim().to_string()),
        Err(err) => {
            eprintln!(
                "warning: agent token unavailable ({AGENT_TOKEN_LOCAL}: {err}); using unauthenticated agent request"
            );
            Ok(String::new())
        }
    }
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

pub(crate) fn agent_stream_request(
    cmd: &str,
    args: Value,
    payload: &[u8],
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
    write_agent_stream_payload(stream, payload, start)
}

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
}
