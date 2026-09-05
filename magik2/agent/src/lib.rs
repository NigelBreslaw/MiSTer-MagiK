//! Bounded native control framing for the independently owned MagiK 2.0 agent.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Envelope {
    pub id: String,
    pub op: String,
    pub token: String,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    Io(String),
    HeaderTooLarge,
    BodyTooLarge,
    Json(String),
}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub fn write_frame(
    writer: &mut impl Write,
    header: &Envelope,
    body: &[u8],
) -> Result<(), FrameError> {
    let encoded =
        serde_json::to_vec(header).map_err(|error| FrameError::Json(error.to_string()))?;
    if encoded.len() > MAX_HEADER_BYTES {
        return Err(FrameError::HeaderTooLarge);
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(FrameError::BodyTooLarge);
    }
    writer.write_all(&(encoded.len() as u32).to_be_bytes())?;
    writer.write_all(&(body.len() as u64).to_be_bytes())?;
    writer.write_all(&encoded)?;
    writer.write_all(body)?;
    Ok(())
}

pub fn read_frame(reader: &mut impl Read) -> Result<(Envelope, Vec<u8>), FrameError> {
    let mut lengths = [0_u8; 12];
    reader.read_exact(&mut lengths)?;
    let header_length = u32::from_be_bytes(lengths[..4].try_into().expect("four bytes")) as usize;
    let body_length = u64::from_be_bytes(lengths[4..].try_into().expect("eight bytes")) as usize;
    if header_length > MAX_HEADER_BYTES {
        return Err(FrameError::HeaderTooLarge);
    }
    if body_length > MAX_BODY_BYTES {
        return Err(FrameError::BodyTooLarge);
    }
    let mut header = vec![0; header_length];
    let mut body = vec![0; body_length];
    reader.read_exact(&mut header)?;
    reader.read_exact(&mut body)?;
    let envelope =
        serde_json::from_slice(&header).map_err(|error| FrameError::Json(error.to_string()))?;
    Ok((envelope, body))
}

#[derive(Debug, Serialize)]
pub struct Status<'a> {
    pub identity: &'a str,
    pub capabilities: &'a [&'a str],
}

/// Single-device native service state. It owns only the 2.0 installation root.
pub struct Agent {
    identity: String,
    token: String,
    install_root: PathBuf,
    state_root: PathBuf,
    process: Mutex<Option<Child>>,
}

impl Agent {
    pub fn new(identity: String, token: String, install_root: PathBuf) -> Self {
        Self::with_state_root(
            identity,
            token,
            install_root,
            PathBuf::from("/tmp/mister-magik2"),
        )
    }

    pub fn with_state_root(
        identity: String,
        token: String,
        install_root: PathBuf,
        state_root: PathBuf,
    ) -> Self {
        Self {
            identity,
            token,
            install_root,
            state_root,
            process: Mutex::new(None),
        }
    }

    pub fn capabilities() -> &'static [&'static str] {
        &["status", "upload-v1", "lifecycle-v1"]
    }

    pub fn handle(&self, stream: &mut TcpStream) -> Result<(), FrameError> {
        let (request, body) = read_frame(stream)?;
        let response = if request.token != self.token {
            response(
                &request.id,
                "error",
                serde_json::json!({"code":"authentication-failed"}),
            )
        } else {
            match request.op.as_str() {
                "status" => response(
                    &request.id,
                    "status",
                    serde_json::json!({
                        "identity": self.identity,
                        "capabilities": Self::capabilities(),
                        "running": self.running(),
                        "artifact": "probe",
                        "artifact_sha256": installed_hash(&self.install_root.join("probe")),
                    }),
                ),
                "upload" => self.upload(&request, &body),
                "start" => self.start(&request),
                "stop" => self.stop(&request),
                _ => response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"unsupported-operation"}),
                ),
            }
        };
        write_frame(stream, &response, &[])
    }

    fn running(&self) -> bool {
        let mut process = self.process.lock().expect("agent process state poisoned");
        let active = process
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_none());
        if !active {
            *process = None;
        }
        active
    }

    fn start(&self, request: &Envelope) -> Envelope {
        let restart = request
            .fields
            .get("restart")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if self.running() && !restart {
            return response(
                &request.id,
                "started",
                serde_json::json!({"already_running":true,"ready":true}),
            );
        }
        let artifact = request
            .fields
            .get("artifact")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("probe");
        if artifact != "probe" {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"unsupported-application"}),
            );
        }
        let executable = self.install_root.join(artifact);
        if !executable.is_file() {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"missing-application"}),
            );
        }
        if let Err(error) = fs::remove_file(self.readiness_path()) {
            if error.kind() != io::ErrorKind::NotFound {
                return response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"readiness-clear-failed","detail":error.to_string()}),
                );
            }
        }
        if restart {
            self.stop_owned_process();
        }
        if let Err(error) = main_handoff("mister_magik_suspend\n") {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"main-suspend-failed","detail":error}),
            );
        }
        match Command::new(executable).spawn() {
            Ok(child) => self.wait_for_readiness(request, child),
            Err(error) => {
                let recovery = main_handoff("mister_magik_resume\n");
                response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"start-failed","detail":error.to_string(),"recovery":recovery.err()}),
                )
            }
        }
    }

    fn stop(&self, request: &Envelope) -> Envelope {
        self.stop_owned_process();
        match main_handoff("mister_magik_resume\n") {
            Ok(()) => response(
                &request.id,
                "stopped",
                serde_json::json!({"launcher_resumed":true}),
            ),
            Err(error) => response(
                &request.id,
                "error",
                serde_json::json!({"code":"launcher-resume-failed","detail":error}),
            ),
        }
    }

    fn upload(&self, request: &Envelope, body: &[u8]) -> Envelope {
        let artifact = request
            .fields
            .get("artifact")
            .and_then(serde_json::Value::as_str);
        let expected_hash = request
            .fields
            .get("sha256")
            .and_then(serde_json::Value::as_str);
        let Some(artifact) = artifact else {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"missing-artifact"}),
            );
        };
        let Some(expected_hash) = expected_hash else {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"missing-sha256"}),
            );
        };
        match publish_atomically(&self.install_root, artifact, expected_hash, body) {
            Ok(()) => response(
                &request.id,
                "uploaded",
                serde_json::json!({"artifact":artifact,"sha256":expected_hash}),
            ),
            Err(error) => response(
                &request.id,
                "error",
                serde_json::json!({"code":"upload-failed","detail":error}),
            ),
        }
    }

    fn readiness_path(&self) -> PathBuf {
        self.state_root.join("probe-ready.json")
    }

    fn stop_owned_process(&self) {
        let mut process = self.process.lock().expect("agent process state poisoned");
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn wait_for_readiness(&self, request: &Envelope, mut child: Child) -> Envelope {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if self.readiness_path().is_file() {
                *self.process.lock().expect("agent process state poisoned") = Some(child);
                return response(
                    &request.id,
                    "started",
                    serde_json::json!({"already_running":false,"ready":true}),
                );
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return self.failed_start(request, "startup-exited", Some(status), &mut child);
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Ok(None) => return self.failed_start(request, "startup-timeout", None, &mut child),
                Err(error) => {
                    return self.failed_start(request, &error.to_string(), None, &mut child);
                }
            }
        }
    }

    fn failed_start(
        &self,
        request: &Envelope,
        code: &str,
        status: Option<ExitStatus>,
        child: &mut Child,
    ) -> Envelope {
        let _ = child.kill();
        let _ = child.wait();
        let recovery = main_handoff("mister_magik_resume\n");
        response(
            &request.id,
            "error",
            serde_json::json!({
                "code":code,
                "exit_status":status.and_then(|value| value.code()),
                "recovery":recovery.err(),
            }),
        )
    }
}

fn main_handoff(command: &str) -> Result<(), String> {
    let mut fifo = File::options()
        .write(true)
        .open("/dev/MiSTer_cmd")
        .map_err(|error| error.to_string())?;
    fifo.write_all(command.as_bytes())
        .map_err(|error| error.to_string())
}

fn response(id: &str, op: &str, value: serde_json::Value) -> Envelope {
    let mut fields = value.as_object().cloned().expect("responses are objects");
    Envelope {
        id: id.to_owned(),
        op: op.to_owned(),
        token: String::new(),
        fields: std::mem::take(&mut fields),
    }
}

fn installed_hash(path: &Path) -> Option<String> {
    Some(
        Sha256::digest(fs::read(path).ok()?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn publish_atomically(
    root: &Path,
    artifact: &str,
    expected_hash: &str,
    body: &[u8],
) -> Result<(), String> {
    let name = Path::new(artifact);
    if name.file_name().and_then(|value| value.to_str()) != Some(artifact) || artifact.is_empty() {
        return Err("artifact must be one plain filename".to_owned());
    }
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let temporary = root.join(format!(".{artifact}.part"));
    let final_path = root.join(artifact);
    let result = (|| -> Result<(), String> {
        let mut output = File::create(&temporary).map_err(|error| error.to_string())?;
        output.write_all(body).map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        let actual_hash = Sha256::digest(body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual_hash != expected_hash {
            return Err("sha256 mismatch".to_owned());
        }
        fs::rename(&temporary, &final_path).map_err(|error| error.to_string())?;
        fs::set_permissions(final_path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_binary_payload() {
        let envelope = Envelope {
            id: "one".into(),
            op: "upload".into(),
            token: "token".into(),
            fields: serde_json::Map::new(),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &envelope, &[0, 255]).expect("write frame");
        assert_eq!(
            read_frame(&mut bytes.as_slice()).expect("read frame"),
            (envelope, vec![0, 255])
        );
    }

    #[test]
    fn rejects_truncated_payload() {
        let error =
            read_frame(&mut &b"\0\0\0\x02\0\0\0\0\0\0\0\x04{}x"[..]).expect_err("truncated body");
        assert!(matches!(error, FrameError::Io(_)));
    }

    #[test]
    fn corrupt_upload_is_never_published() {
        let directory =
            std::env::temp_dir().join(format!("magik2-agent-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        assert!(publish_atomically(&directory, "probe", "bad", b"content").is_err());
        assert!(!directory.join("probe").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn installed_hash_tracks_the_published_artifact() {
        let directory =
            std::env::temp_dir().join(format!("magik2-agent-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create test directory");
        let artifact = directory.join("probe");
        std::fs::write(&artifact, b"probe payload").expect("write artifact");
        let expected = Sha256::digest(b"probe payload")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(installed_hash(&artifact), Some(expected));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn native_service_authenticates_and_publishes_a_verified_payload() {
        let directory =
            std::env::temp_dir().join(format!("magik2-agent-loopback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("listener address");
        let agent = Agent::new(
            "other-branch".to_owned(),
            "token".to_owned(),
            directory.clone(),
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");
            agent.handle(&mut stream).expect("handle upload");
        });
        let body = b"probe payload";
        let hash = Sha256::digest(body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut fields = serde_json::Map::new();
        fields.insert("artifact".to_owned(), serde_json::json!("probe"));
        fields.insert("sha256".to_owned(), serde_json::json!(hash));
        let mut client = std::net::TcpStream::connect(address).expect("connect agent");
        write_frame(
            &mut client,
            &Envelope {
                id: "one".into(),
                op: "upload".into(),
                token: "token".into(),
                fields,
            },
            body,
        )
        .expect("write upload");
        assert_eq!(
            read_frame(&mut client).expect("read response").0.op,
            "uploaded"
        );
        server.join().expect("server thread");
        assert_eq!(
            std::fs::read(directory.join("probe")).expect("published probe"),
            body
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
