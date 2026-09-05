//! Bounded native control framing for the independently owned MagiK 2.0 agent.

use mister_magik_framebuffer_stream::read_frame as read_preview_frame;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Envelope {
    pub id: String,
    pub op: String,
    pub token: String,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct OwnedProcess {
    pid: u32,
    executable: String,
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
    mutations: Mutex<()>,
    replayed_mutations: Mutex<VecDeque<ReplayedMutation>>,
    observation: Arc<Observation>,
}

const MAX_RECENT_LOGS: usize = 100;
const MAX_REPLAYED_MUTATIONS: usize = 64;
const WATCH_INTERVAL: Duration = Duration::from_millis(200);
const VIEWER_LEASE: Duration = Duration::from_secs(2);
const TEST_SESSION_DEADLINE: Duration = Duration::from_secs(20);
const SESSION_IO_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Clone)]
struct ReplayedMutation {
    request_id: String,
    fingerprint: String,
    response: Envelope,
}

#[derive(Default)]
struct ObservationState {
    latest_frame: Option<Vec<u8>>,
    latest_frame_sequence: u64,
    logs: VecDeque<(u64, String)>,
    next_log_sequence: u64,
}

#[derive(Default)]
struct Observation {
    state: Mutex<ObservationState>,
}

impl Observation {
    fn record_log(&self, line: String) {
        let mut state = self.state.lock().expect("observation state poisoned");
        state.next_log_sequence += 1;
        let sequence = state.next_log_sequence;
        state.logs.push_back((sequence, line));
        while state.logs.len() > MAX_RECENT_LOGS {
            state.logs.pop_front();
        }
    }

    fn record_frame(&self, sequence: u64, bytes: Vec<u8>) {
        let mut state = self.state.lock().expect("observation state poisoned");
        if sequence >= state.latest_frame_sequence {
            state.latest_frame_sequence = sequence;
            state.latest_frame = Some(bytes);
        }
    }

    fn snapshot(
        &self,
        after_log: u64,
        after_frame: u64,
    ) -> (Vec<(u64, String)>, Option<(u64, Vec<u8>)>) {
        let state = self.state.lock().expect("observation state poisoned");
        let logs = state
            .logs
            .iter()
            .filter(|(sequence, _)| *sequence > after_log)
            .cloned()
            .collect();
        let frame = (state.latest_frame_sequence > after_frame)
            .then(|| {
                state
                    .latest_frame
                    .clone()
                    .map(|frame| (state.latest_frame_sequence, frame))
            })
            .flatten();
        (logs, frame)
    }
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
            mutations: Mutex::new(()),
            replayed_mutations: Mutex::new(VecDeque::new()),
            observation: Arc::default(),
        }
    }

    pub fn capabilities() -> &'static [&'static str] {
        &[
            "status",
            "upload-v1",
            "lifecycle-v1",
            "test-bridge-v1",
            "metrics-v1",
            "watch-v1",
            "artifacts-v1",
            "agent-update-v1",
            "request-replay-v1",
            "test-deadline-v1",
            "legacy-isolation-v1",
        ]
    }

    /// Starts the probe-produced frame receiver. This socket never reads a
    /// framebuffer device: the owned application publishes already-rendered
    /// previews and the agent retains only the newest complete frame.
    pub fn start_observation_receiver(&self) -> Result<(), String> {
        fs::create_dir_all(&self.state_root).map_err(|error| error.to_string())?;
        let socket_path = self.state_root.join("probe-frames.sock");
        if socket_path.exists() {
            fs::remove_file(&socket_path).map_err(|error| error.to_string())?;
        }
        let listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
        let observation = self.observation.clone();
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                match connection {
                    Ok(connection) => {
                        let observation = observation.clone();
                        std::thread::spawn(move || receive_preview_frames(connection, observation));
                    }
                    Err(error) => eprintln!("magik2 preview accept failed: {error}"),
                }
            }
        });
        Ok(())
    }

    pub fn handle(&self, stream: &mut TcpStream) -> Result<(), FrameError> {
        let (request, body) = read_frame(stream)?;
        if request.token != self.token {
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"authentication-failed"}),
                ),
                &[],
            );
        }

        if request.op == "watch" {
            return self.watch(stream, &request, &body);
        }
        if request.op == "read-artifact" {
            return self.read_artifact(stream, &request, &body);
        }
        if request.op == "test-start" {
            let _mutation = self.mutations.lock().expect("mutation state poisoned");
            return self.start_test_session(stream, &request, &body);
        }
        if request.op == "agent-update" {
            let _mutation = self.mutations.lock().expect("mutation state poisoned");
            return self.upgrade_agent(stream, &request, &body);
        }

        let replayable = matches!(request.op.as_str(), "upload" | "start" | "stop");
        let _mutation = replayable.then(|| self.mutations.lock().expect("mutation state poisoned"));
        if replayable {
            match self.replayed_response(&request, &body) {
                Ok(Some(response)) => return write_frame(stream, &response, &[]),
                Ok(None) => {}
                Err(response) => return write_frame(stream, &response, &[]),
            }
        }
        let response = match request.op.as_str() {
            "status" => response(
                &request.id,
                "status",
                serde_json::json!({
                    "identity": self.identity,
                    "capabilities": Self::capabilities(),
                    "running": self.running(),
                    "artifact": "probe",
                    "artifact_sha256": installed_hash(&self.install_root.join("probe")),
                    "legacy_agent_running": legacy_agent_running(),
                }),
            ),
            "upload" => self.upload(&request, &body),
            "start" => self.start(&request),
            "stop" => self.stop(&request),
            "metrics" => self.metrics(&request),
            _ => response(
                &request.id,
                "error",
                serde_json::json!({"code":"unsupported-operation"}),
            ),
        };
        if replayable {
            self.remember_response(&request, &body, &response);
        }
        write_frame(stream, &response, &[])
    }

    fn replayed_response(
        &self,
        request: &Envelope,
        body: &[u8],
    ) -> Result<Option<Envelope>, Envelope> {
        let fingerprint = mutation_fingerprint(request, body);
        let replayed = self
            .replayed_mutations
            .lock()
            .expect("mutation replay state poisoned");
        let Some(previous) = replayed
            .iter()
            .find(|previous| previous.request_id == request.id)
        else {
            return Ok(None);
        };
        if previous.fingerprint == fingerprint {
            Ok(Some(previous.response.clone()))
        } else {
            Err(response(
                &request.id,
                "error",
                serde_json::json!({"code":"request-id-reused"}),
            ))
        }
    }

    fn remember_response(&self, request: &Envelope, body: &[u8], response: &Envelope) {
        let mut replayed = self
            .replayed_mutations
            .lock()
            .expect("mutation replay state poisoned");
        replayed.push_back(ReplayedMutation {
            request_id: request.id.clone(),
            fingerprint: mutation_fingerprint(request, body),
            response: response.clone(),
        });
        while replayed.len() > MAX_REPLAYED_MUTATIONS {
            replayed.pop_front();
        }
    }

    fn running(&self) -> bool {
        let mut process = self.process.lock().expect("agent process state poisoned");
        let has_owned_child = process.is_some();
        let active = process
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_none());
        if has_owned_child && !active {
            *process = None;
            self.clear_owned_process();
        }
        active || self.owned_process_is_running()
    }

    fn start(&self, request: &Envelope) -> Envelope {
        self.start_with_test_server(request, None)
    }

    fn start_with_test_server(&self, request: &Envelope, test_server: Option<String>) -> Envelope {
        let restart = request
            .fields
            .get("restart")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            || test_server.is_some();
        if self.running() && !restart && test_server.is_none() {
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
        let mut command = Command::new(executable);
        command
            .env("MISTER_MAGIK2_STATE_ROOT", &self.state_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(test_server) = test_server {
            command.env("SLINT_TEST_SERVER", test_server);
        }
        if let Some(profile_id) = request
            .fields
            .get("profile_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| is_plain_name(value))
        {
            command.env(
                "MISTER_MAGIK2_PROFILE_DIR",
                self.state_root.join("profiles").join(profile_id),
            );
        }
        match command.spawn() {
            Ok(mut child) => {
                if let Some(stdout) = child.stdout.take() {
                    forward_child_logs(stdout, self.observation.clone());
                }
                if let Some(stderr) = child.stderr.take() {
                    forward_child_logs(stderr, self.observation.clone());
                }
                self.wait_for_readiness(request, child)
            }
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

    fn upgrade_agent(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
        body: &[u8],
    ) -> Result<(), FrameError> {
        let expected_hash = request
            .fields
            .get("sha256")
            .and_then(serde_json::Value::as_str);
        let Some(expected_hash) = expected_hash else {
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"missing-sha256"}),
                ),
                &[],
            );
        };
        if let Err(error) = publish_atomically(
            &self.install_root,
            "mister-magik2-agent",
            expected_hash,
            body,
        ) {
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"agent-update-failed","detail":error}),
                ),
                &[],
            );
        }
        write_frame(
            stream,
            &response(
                &request.id,
                "agent-updating",
                serde_json::json!({"sha256":expected_hash}),
            ),
            &[],
        )?;
        let error = Command::new(self.install_root.join("mister-magik2-agent"))
            .env("MISTER_MAGIK2_TOKEN", &self.token)
            .env("MISTER_MAGIK2_INSTALL_ROOT", &self.install_root)
            .env("MISTER_MAGIK2_STATE_ROOT", &self.state_root)
            .exec();
        Err(FrameError::Io(format!(
            "agent replacement exec failed: {error}"
        )))
    }

    fn metrics(&self, request: &Envelope) -> Envelope {
        match self.read_metrics() {
            Ok(value) if value.is_object() => response(&request.id, "metrics", value),
            Ok(_) => response(
                &request.id,
                "error",
                serde_json::json!({"code":"invalid-metrics"}),
            ),
            Err(error) => response(
                &request.id,
                "error",
                serde_json::json!({"code":"metrics-unavailable","detail":error}),
            ),
        }
    }

    fn read_metrics(&self) -> Result<serde_json::Value, String> {
        fs::read(self.state_root.join("probe-metrics.json"))
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
    }

    fn watch(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
        body: &[u8],
    ) -> Result<(), FrameError> {
        if !body.is_empty() {
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"watch-has-no-body"}),
                ),
                &[],
            );
        }
        stream
            .set_write_timeout(Some(Duration::from_millis(500)))
            .map_err(FrameError::from)?;
        write_frame(
            stream,
            &response(
                &request.id,
                "watch-ready",
                serde_json::json!({"ready":true}),
            ),
            &[],
        )?;
        let mut log_sequence = 0;
        let mut frame_sequence = 0;
        loop {
            self.renew_viewer_lease();
            if let Ok(metrics) = self.read_metrics() {
                write_frame(
                    stream,
                    &response(
                        &request.id,
                        "watch-metrics",
                        serde_json::json!({"metrics":metrics}),
                    ),
                    &[],
                )?;
            }
            let (logs, frame) = self.observation.snapshot(log_sequence, frame_sequence);
            for (sequence, line) in logs {
                write_frame(
                    stream,
                    &response(
                        &request.id,
                        "watch-log",
                        serde_json::json!({"sequence":sequence,"line":line}),
                    ),
                    &[],
                )?;
                log_sequence = sequence;
            }
            if let Some((sequence, bytes)) = frame {
                write_frame(
                    stream,
                    &response(
                        &request.id,
                        "watch-frame",
                        serde_json::json!({"sequence":sequence}),
                    ),
                    &bytes,
                )?;
                frame_sequence = sequence;
            }
            std::thread::sleep(WATCH_INTERVAL);
        }
    }

    fn renew_viewer_lease(&self) {
        let deadline = std::time::SystemTime::now()
            .checked_add(VIEWER_LEASE)
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_millis());
        let temporary = self.state_root.join("viewer-lease.next");
        let destination = self.state_root.join("viewer-lease");
        if fs::write(&temporary, deadline.to_string()).is_ok() {
            let _ = fs::rename(temporary, destination);
        }
    }

    fn read_artifact(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
        body: &[u8],
    ) -> Result<(), FrameError> {
        let profile_id = request
            .fields
            .get("profile_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| is_plain_name(value));
        let name = request
            .fields
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|value| is_plain_name(value));
        let Some(profile_id) = profile_id else {
            return artifact_request_error(stream, request, body);
        };
        let Some(name) = name else {
            return artifact_request_error(stream, request, body);
        };
        if !body.is_empty() {
            return artifact_request_error(stream, request, body);
        }
        match fs::read(self.state_root.join("profiles").join(profile_id).join(name)) {
            Ok(bytes) => write_frame(
                stream,
                &response(
                    &request.id,
                    "artifact",
                    serde_json::json!({"profile_id":profile_id,"name":name}),
                ),
                &bytes,
            ),
            Err(error) => write_frame(
                stream,
                &response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"artifact-unavailable","detail":error.to_string()}),
                ),
                &[],
            ),
        }
    }

    fn readiness_path(&self) -> PathBuf {
        self.state_root.join("probe-ready.json")
    }

    fn owned_process_path(&self) -> PathBuf {
        self.state_root.join("owned-process.json")
    }

    fn write_owned_process(&self, pid: u32) -> Result<(), String> {
        let record = OwnedProcess {
            pid,
            executable: self.install_root.join("probe").display().to_string(),
        };
        let temporary = self.owned_process_path().with_extension("next");
        let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
        fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        fs::rename(temporary, self.owned_process_path()).map_err(|error| error.to_string())
    }

    fn read_owned_process(&self) -> Option<OwnedProcess> {
        serde_json::from_slice(&fs::read(self.owned_process_path()).ok()?).ok()
    }

    fn clear_owned_process(&self) {
        let _ = fs::remove_file(self.owned_process_path());
    }

    fn owned_process_is_running(&self) -> bool {
        let record = self
            .read_owned_process()
            .or_else(|| self.discover_owned_process());
        let Some(record) = record else { return false };
        let Ok(command) = fs::read(format!("/proc/{}/cmdline", record.pid)) else {
            self.clear_owned_process();
            return false;
        };
        let executable = command.split(|byte| *byte == 0).next().unwrap_or_default();
        if executable == record.executable.as_bytes() {
            true
        } else {
            self.clear_owned_process();
            false
        }
    }

    fn discover_owned_process(&self) -> Option<OwnedProcess> {
        let executable = self.install_root.join("probe").display().to_string();
        let mut matches = fs::read_dir("/proc")
            .ok()?
            .flatten()
            .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
            .filter(|pid| {
                fs::read(format!("/proc/{pid}/cmdline"))
                    .ok()
                    .and_then(|command| {
                        command
                            .split(|byte| *byte == 0)
                            .next()
                            .map(ToOwned::to_owned)
                    })
                    .is_some_and(|command| command == executable.as_bytes())
            });
        let pid = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let record = OwnedProcess { pid, executable };
        let _ = self.write_owned_process(record.pid);
        Some(record)
    }

    fn stop_owned_process(&self) {
        let mut process = self.process.lock().expect("agent process state poisoned");
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.clear_owned_process();
            return;
        }
        drop(process);
        if let Some(record) = self.read_owned_process() {
            // The record is accepted only when /proc confirms it still runs our
            // exact executable, preventing a recycled PID from being signalled.
            if self.owned_process_is_running() {
                // SAFETY: `kill` is called with a validated positive PID and a
                // fixed SIGTERM value; no pointers are passed across the FFI.
                let _ = unsafe { libc::kill(record.pid as libc::pid_t, libc::SIGTERM) };
                let deadline = Instant::now() + Duration::from_secs(2);
                while self.owned_process_is_running() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
            self.clear_owned_process();
        }
    }

    fn start_test_session(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
        body: &[u8],
    ) -> Result<(), FrameError> {
        if !body.is_empty() {
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"test-session-has-no-body"}),
                ),
                &[],
            );
        }
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) => {
                return write_frame(
                    stream,
                    &response(
                        &request.id,
                        "error",
                        serde_json::json!({"code":"test-bridge-bind-failed","detail":error.to_string()}),
                    ),
                    &[],
                );
            }
        };
        let endpoint = listener.local_addr().map_err(FrameError::from)?.to_string();
        let started = self.start_with_test_server(request, Some(endpoint));
        if started.op == "error" {
            return write_frame(stream, &started, &[]);
        }
        write_frame(
            stream,
            &response(&request.id, "test-ready", serde_json::json!({"ready":true})),
            &[],
        )?;
        self.forward_test_connection(listener, stream)
    }

    fn forward_test_connection(
        &self,
        listener: TcpListener,
        stream: &mut TcpStream,
    ) -> Result<(), FrameError> {
        listener.set_nonblocking(true).map_err(FrameError::from)?;
        let deadline = Instant::now() + TEST_SESSION_DEADLINE;
        let mut application = loop {
            match listener.accept() {
                Ok((connection, _)) => break connection,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.stop_owned_process();
                    return Err(FrameError::Io(
                        "test application did not connect before deadline".to_owned(),
                    ));
                }
                Err(error) => {
                    self.stop_owned_process();
                    return Err(FrameError::from(error));
                }
            }
        };
        application
            .set_read_timeout(Some(SESSION_IO_TIMEOUT))
            .map_err(FrameError::from)?;
        application
            .set_write_timeout(Some(SESSION_IO_TIMEOUT))
            .map_err(FrameError::from)?;
        stream
            .set_read_timeout(Some(SESSION_IO_TIMEOUT))
            .map_err(FrameError::from)?;
        stream
            .set_write_timeout(Some(SESSION_IO_TIMEOUT))
            .map_err(FrameError::from)?;
        let mut device_to_host = application.try_clone().map_err(FrameError::from)?;
        let mut host_clone = stream.try_clone().map_err(FrameError::from)?;
        let upstream = std::thread::spawn(move || {
            let _ = relay_until_deadline(&mut device_to_host, &mut host_clone, deadline);
            let _ = host_clone.shutdown(Shutdown::Write);
        });
        let primary = relay_until_deadline(stream, &mut application, deadline);
        let _ = application.shutdown(Shutdown::Write);
        let _ = upstream.join();
        self.stop_owned_process();
        primary
    }

    fn wait_for_readiness(&self, request: &Envelope, mut child: Child) -> Envelope {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if self.readiness_path().is_file() {
                if let Err(error) = self.write_owned_process(child.id()) {
                    return self.failed_start(request, &error, None, &mut child);
                }
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

fn forward_child_logs(reader: impl Read + Send + 'static, observation: Arc<Observation>) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => observation.record_log(line),
                Err(error) => {
                    observation.record_log(format!("probe log read failed: {error}"));
                    return;
                }
            }
        }
    });
}

fn receive_preview_frames(mut stream: UnixStream, observation: Arc<Observation>) {
    loop {
        let mut encoded = Vec::new();
        match read_preview_frame(&mut stream) {
            Ok((header, payload)) => {
                let capacity = mister_magik_framebuffer_stream::HEADER_LEN + payload.len();
                if encoded.try_reserve_exact(capacity).is_err() {
                    observation.record_log("probe preview allocation failed".to_owned());
                    return;
                }
                encoded.extend_from_slice(&header.encode());
                encoded.extend_from_slice(&payload);
                observation.record_frame(header.sequence, encoded);
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return,
            Err(error) => {
                observation.record_log(format!("probe preview rejected: {error}"));
                return;
            }
        }
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

fn is_plain_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && Path::new(value).file_name().and_then(|name| name.to_str()) == Some(value)
}

fn artifact_request_error(
    stream: &mut TcpStream,
    request: &Envelope,
    _body: &[u8],
) -> Result<(), FrameError> {
    write_frame(
        stream,
        &response(
            &request.id,
            "error",
            serde_json::json!({"code":"invalid-artifact-request"}),
        ),
        &[],
    )
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

fn mutation_fingerprint(request: &Envelope, body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.op.as_bytes());
    hasher.update([0]);
    hasher
        .update(serde_json::to_vec(&request.fields).expect("request fields are serializable JSON"));
    hasher.update([0]);
    hasher.update(body);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn relay_until_deadline(
    source: &mut impl Read,
    destination: &mut impl Write,
    deadline: Instant,
) -> Result<(), FrameError> {
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if Instant::now() >= deadline {
            return Err(FrameError::Io("test session deadline elapsed".to_owned()));
        }
        match source.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(size) => destination
                .write_all(&buffer[..size])
                .map_err(FrameError::from)?,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(FrameError::from(error)),
        }
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

fn legacy_agent_running() -> bool {
    let Ok(processes) = fs::read_dir("/proc") else {
        return false;
    };
    processes.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
            && fs::read_to_string(entry.path().join("comm"))
                .ok()
                .is_some_and(|name| name.trim() == "mister-magik-agent")
    })
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
    fn artifact_names_cannot_escape_the_profile_root() {
        assert!(is_plain_name("flamegraph.svg"));
        assert!(!is_plain_name(""));
        assert!(!is_plain_name("../token"));
        assert!(!is_plain_name("nested/profile.folded"));
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

    #[test]
    fn repeated_mutation_identifier_replays_without_republishing() {
        let directory =
            std::env::temp_dir().join(format!("magik2-agent-replay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("listener address");
        let agent = std::sync::Arc::new(Agent::new(
            "other-branch".to_owned(),
            "token".to_owned(),
            directory.clone(),
        ));
        let server_agent = agent.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().expect("accept client");
                server_agent.handle(&mut stream).expect("handle upload");
            }
        });
        let body = b"probe payload";
        let hash = Sha256::digest(body)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut fields = serde_json::Map::new();
        fields.insert("artifact".to_owned(), serde_json::json!("probe"));
        fields.insert("sha256".to_owned(), serde_json::json!(hash));
        let request = Envelope {
            id: "lost-reply".into(),
            op: "upload".into(),
            token: "token".into(),
            fields,
        };
        let mut first = std::net::TcpStream::connect(address).expect("connect first client");
        write_frame(&mut first, &request, body).expect("write first upload");
        assert_eq!(
            read_frame(&mut first).expect("read first reply").0.op,
            "uploaded"
        );
        std::fs::write(directory.join("probe"), b"already handled")
            .expect("change published probe");
        let mut retry = std::net::TcpStream::connect(address).expect("connect retry client");
        write_frame(&mut retry, &request, body).expect("write retried upload");
        assert_eq!(
            read_frame(&mut retry).expect("read replayed reply").0.op,
            "uploaded"
        );
        let mut reused = std::net::TcpStream::connect(address).expect("connect mismatched retry");
        write_frame(&mut reused, &request, b"different body").expect("write mismatched retry");
        assert_eq!(
            read_frame(&mut reused).expect("read rejected retry").0.op,
            "error"
        );
        server.join().expect("server thread");
        assert_eq!(
            std::fs::read(directory.join("probe")).expect("read probe"),
            b"already handled"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn continuous_traffic_cannot_extend_a_test_session_deadline() {
        struct BusyReader;
        impl Read for BusyReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                buffer[0] = 1;
                Ok(1)
            }
        }
        let mut reader = BusyReader;
        let mut output = Vec::new();
        let result = relay_until_deadline(
            &mut reader,
            &mut output,
            Instant::now() + Duration::from_millis(5),
        );
        assert!(
            matches!(result, Err(FrameError::Io(message)) if message == "test session deadline elapsed")
        );
        assert!(!output.is_empty());
    }

    #[test]
    fn test_relay_returns_when_a_client_disconnects() {
        let mut disconnected = io::Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        assert!(
            relay_until_deadline(
                &mut disconnected,
                &mut output,
                Instant::now() + Duration::from_secs(1),
            )
            .is_ok()
        );
    }

    #[test]
    fn test_relay_reports_an_application_failure() {
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "application exited",
                ))
            }
        }
        let mut failed = FailedReader;
        let mut output = Vec::new();
        assert!(matches!(
            relay_until_deadline(
                &mut failed,
                &mut output,
                Instant::now() + Duration::from_secs(1),
            ),
            Err(FrameError::Io(message)) if message.contains("application exited")
        ));
    }

    #[test]
    fn slow_watch_does_not_block_a_separate_control_request() {
        let directory =
            std::env::temp_dir().join(format!("magik2-agent-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create state directory");
        std::fs::write(directory.join("probe-metrics.json"), b"{}").expect("write metrics");
        let agent = std::sync::Arc::new(Agent::with_state_root(
            "test".to_owned(),
            "token".to_owned(),
            directory.join("install"),
            directory.clone(),
        ));
        agent.observation.record_frame(1, vec![0; 4 * 1024 * 1024]);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("listener address");
        let (finished_sender, finished_receiver) = std::sync::mpsc::channel();
        let server_agent = agent.clone();
        let server = std::thread::spawn(move || {
            let (mut watch, _) = listener.accept().expect("accept watch");
            let watching_agent = server_agent.clone();
            let watching = std::thread::spawn(move || watching_agent.handle(&mut watch));
            let (mut status, _) = listener.accept().expect("accept status");
            server_agent.handle(&mut status).expect("handle status");
            finished_sender
                .send(watching.join().expect("watch thread"))
                .expect("send watch result");
        });
        let mut slow = std::net::TcpStream::connect(address).expect("connect watch");
        write_frame(
            &mut slow,
            &Envelope {
                id: "watch".into(),
                op: "watch".into(),
                token: "token".into(),
                fields: serde_json::Map::new(),
            },
            &[],
        )
        .expect("start watch");
        assert_eq!(
            read_frame(&mut slow).expect("watch ready").0.op,
            "watch-ready"
        );
        let mut status = std::net::TcpStream::connect(address).expect("connect status");
        write_frame(
            &mut status,
            &Envelope {
                id: "status".into(),
                op: "status".into(),
                token: "token".into(),
                fields: serde_json::Map::new(),
            },
            &[],
        )
        .expect("request status");
        assert_eq!(
            read_frame(&mut status).expect("status reply").0.op,
            "status"
        );
        assert!(
            finished_receiver
                .recv_timeout(Duration::from_secs(3))
                .expect("bounded watch result")
                .is_err()
        );
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(directory);
    }
}
