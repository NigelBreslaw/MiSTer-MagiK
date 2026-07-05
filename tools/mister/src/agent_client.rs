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

pub(crate) struct AgentResponse {
    pub(crate) response: Value,
    pub(crate) elapsed_ms: u128,
}

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

pub(crate) fn agent_binary_request(
    cmd: &str,
    args: Value,
    timeout: Duration,
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
    let raw_bytes = response
        .pointer("/result/raw_bytes")
        .and_then(Value::as_u64)
        .ok_or("agent binary response missing result.raw_bytes")? as usize;
    let mut payload = vec![0u8; raw_bytes];
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
