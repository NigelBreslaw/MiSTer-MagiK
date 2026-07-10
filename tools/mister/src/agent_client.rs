use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::remote::host;
use crate::Result;

pub(crate) const AGENT_PORT: u16 = 7498;
const AGENT_TOKEN_LOCAL: &str = "build/mister-agent.token";
const MAX_AGENT_BINARY_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;

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
    let request = json!({
        "token": token,
        "id": 1,
        "cmd": cmd,
        "args": args,
    });
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
    let request = json!({
        "token": token,
        "id": 1,
        "cmd": cmd,
        "args": args,
    });
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
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
    let request = json!({
        "token": token,
        "id": 1,
        "cmd": cmd,
        "args": args,
    });
    let start = Instant::now();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{request}")?;
    stream.write_all(payload)?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    parse_agent_response_line(line, start)
}

fn parse_agent_response_line(line: String, start: Instant) -> Result<AgentResponse> {
    if line.trim().is_empty() {
        return Err("empty response from agent".into());
    }
    let response: Value = serde_json::from_str(line.trim())?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(AgentResponse {
            response,
            elapsed_ms: start.elapsed().as_millis(),
        })
    } else {
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("agent command failed");
        Err(error.to_string().into())
    }
}

fn agent_binary_payload_len(response: &Value) -> Result<usize> {
    let payload_bytes = response
        .pointer("/result/payload_bytes")
        .or_else(|| response.pointer("/result/raw_bytes"))
        .and_then(Value::as_u64)
        .ok_or("agent binary response missing result payload byte count")?;
    if payload_bytes > MAX_AGENT_BINARY_PAYLOAD_BYTES {
        return Err(
            format!("agent binary response payload too large: {payload_bytes} bytes").into(),
        );
    }
    usize::try_from(payload_bytes).map_err(|_| {
        format!("agent binary response payload size overflows usize: {payload_bytes}").into()
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
    fn agent_binary_payload_len_rejects_missing_or_oversized_values() {
        assert!(agent_binary_payload_len(&json!({"result": {}}))
            .expect_err("missing payload len should fail")
            .to_string()
            .contains("missing result payload byte count"));
        assert!(agent_binary_payload_len(&json!({
            "result": {
                "payload_bytes": MAX_AGENT_BINARY_PAYLOAD_BYTES + 1,
            }
        }))
        .expect_err("oversized payload len should fail")
        .to_string()
        .contains("payload too large"));
    }
}
