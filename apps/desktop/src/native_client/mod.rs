//! Direct native control; Python is only an exceptional service preparation step.
mod discovery;
pub mod subscription;
pub mod wire;
pub use discovery::Client;
use serde_json::{Value, json};
use std::{
    io::{BufReader, Read},
    net::TcpStream,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
#[derive(Debug)]
pub enum AgentError {
    Unreachable(String),
    Unauthorized,
    Protocol(String),
    Command(String),
}
impl From<std::io::Error> for AgentError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::InvalidData {
            Self::Protocol(e.to_string())
        } else {
            Self::Unreachable(e.to_string())
        }
    }
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "native service rejected authentication"),
            Self::Unreachable(s) | Self::Protocol(s) | Self::Command(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for AgentError {}
static NEXT: AtomicU64 = AtomicU64::new(1);
fn request_id() -> String {
    format!(
        "desktop-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
struct Deadline<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "native request deadline")
            })?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(buf)
    }
}
impl Client {
    pub fn request(&self, op: &str, args: Value) -> Result<Value, AgentError> {
        let (header, body) = self.binary(op, args)?;
        if header["format"] == "json" {
            serde_json::from_slice(&body).map_err(|e| AgentError::Protocol(e.to_string()))
        } else if body.is_empty() {
            Ok(header)
        } else {
            Err(AgentError::Protocol("unexpected binary body".into()))
        }
    }
    pub fn binary(&self, op: &str, args: Value) -> Result<(Value, Vec<u8>), AgentError> {
        let timeout = Duration::from_secs(10);
        let deadline = Instant::now() + timeout;
        let mut stream = self.connect(timeout)?;
        let id = request_id();
        wire::write(&mut stream, &id, op, &self.token, args)?;
        let expected = match op {
            "status" => "status",
            "identify" => "identified",
            "capture-framebuffer" => "framebuffer",
            _ => "",
        };
        let expected = if expected.is_empty() {
            format!("{op}-result")
        } else {
            expected.to_string()
        };
        wire::read(
            &mut Deadline {
                stream: &mut stream,
                deadline,
            },
            &id,
            &expected,
        )
    }
    pub fn subscribe(&self, op: &str) -> Result<(String, BufReader<TcpStream>), AgentError> {
        let mut stream = self.connect(Duration::from_secs(10))?;
        let id = request_id();
        wire::write(&mut stream, &id, op, &self.token, json!({}))?;
        let (_, body) = wire::read(
            &mut Deadline {
                stream: &mut stream,
                deadline: Instant::now() + Duration::from_secs(10),
            },
            &id,
            &format!("{op}-ready"),
        )?;
        if !body.is_empty() {
            return Err(AgentError::Protocol("unexpected subscription body".into()));
        }
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        Ok((id, BufReader::new(stream)))
    }
}
