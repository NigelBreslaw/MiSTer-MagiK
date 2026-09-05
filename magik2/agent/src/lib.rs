//! Bounded native control framing for the independently owned MagiK 2.0 agent.

mod main_control;
mod upload;
mod wire;

use mister_magik_framebuffer_stream::read_frame as read_preview_frame;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use wire::{Envelope, FrameError, MAX_BODY_BYTES, MAX_HEADER_BYTES, read_frame, write_frame};
#[derive(Debug, Deserialize, Serialize)]
struct OwnedProcess {
    pid: u32,
    executable: String,
    #[serde(default)]
    start_ticks: String,
    #[serde(default)]
    sha256: String,
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
const TEST_APPLICATION_CONNECT_DEADLINE: Duration = Duration::from_secs(20);
const TEST_SESSION_DEADLINE: Duration = Duration::from_secs(60);
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

type ObservationSnapshot = (Vec<(u64, String)>, Option<(u64, Vec<u8>)>);

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

    fn record_frame(&self, _sequence: u64, bytes: Vec<u8>) {
        let mut state = self.state.lock().expect("observation state poisoned");
        state.latest_frame_sequence += 1;
        state.latest_frame = Some(bytes);
    }

    fn snapshot(&self, after_log: u64, after_frame: u64) -> ObservationSnapshot {
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
            "diagnostics",
            "upload-v1",
            "lifecycle-v1",
            "start-artifact",
            "test-bridge-v1",
            "test-session",
            "metrics-v1",
            "watch-v1",
            "artifacts-v1",
            "agent-update-v1",
            "request-replay-v1",
            "test-deadline-v1",
            "test-deadline-v2",
            "legacy-isolation-v1",
        ]
    }

    /// Starts the probe-produced frame receiver. This socket never reads a
    /// framebuffer device: the owned application publishes already-rendered
    /// previews and the agent retains only the newest complete frame.
    pub fn start_observation_receiver(&self) -> Result<(), String> {
        fs::create_dir_all(&self.state_root).map_err(|error| error.to_string())?;
        let root = self.state_root.clone();
        let observation = self.observation.clone();
        std::thread::spawn(move || {
            let mut previous = String::new();
            loop {
                let current = log_tail(&root.join("probe.log"));
                if current != previous {
                    let added = current.strip_prefix(&previous).unwrap_or(&current);
                    for line in added
                        .lines()
                        .rev()
                        .take(MAX_RECENT_LOGS)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        observation.record_log(line.chars().take(2048).collect());
                    }
                    previous = current;
                }
                for name in ["probe.log", "agent.log"] {
                    let path = root.join(name);
                    if fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024)
                        && let Ok(file) = OpenOptions::new().write(true).open(path)
                    {
                        let _ = file.set_len(0);
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
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
                        let _ = connection.set_read_timeout(Some(Duration::from_secs(1)));
                        receive_preview_frames(connection, observation);
                    }
                    Err(error) => eprintln!("magik2 preview accept failed: {error}"),
                }
            }
        });
        Ok(())
    }

    pub fn handle(&self, stream: &mut TcpStream) -> Result<(), FrameError> {
        let deadline = Instant::now() + Duration::from_secs(30);
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let (request, body_length) =
            wire::read_header(&mut wire::DeadlineReader { stream, deadline })?;
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

        if matches!(request.op.as_str(), "upload" | "agent-update") {
            let _mutation = self.mutations.lock().expect("mutation state poisoned");
            let artifact = if request.op == "agent-update" {
                "mister-magik2-agent"
            } else {
                request
                    .fields
                    .get("artifact")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
            };
            let hash = request
                .fields
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let staged = match upload::receive(
                &mut wire::DeadlineReader { stream, deadline },
                &self.install_root,
                artifact,
                hash,
                body_length,
                &request.id,
            ) {
                Ok(staged) => staged,
                Err(error) => {
                    return write_frame(
                        stream,
                        &response(
                            &request.id,
                            "error",
                            serde_json::json!({"code":"upload-failed","detail":error}),
                        ),
                        &[],
                    );
                }
            };
            if request.op == "agent-update" {
                return self.upgrade_agent(stream, &request, staged);
            }
            match self.replayed_response(&request, &[]) {
                Ok(Some(previous)) => return write_frame(stream, &previous, &[]),
                Err(error) => return write_frame(stream, &error, &[]),
                _ => {}
            }
            staged
                .publish(&self.install_root.join(artifact))
                .map_err(FrameError::Io)?;
            let result = response(
                &request.id,
                "uploaded",
                serde_json::json!({"artifact":artifact,"sha256":hash}),
            );
            self.remember_response(&request, &[], &result);
            return write_frame(stream, &result, &[]);
        }
        if body_length > MAX_HEADER_BYTES {
            return Err(FrameError::BodyTooLarge);
        }
        let mut body = vec![0; body_length];
        wire::DeadlineReader { stream, deadline }.read_exact(&mut body)?;

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
        let replayable = matches!(request.op.as_str(), "start" | "stop");
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
                    "agent_pid": std::process::id(),
                    "agent_sha256": installed_hash(&PathBuf::from("/proc/self/exe")),
                    "capabilities": Self::capabilities(),
                    "running": self.running(),
                    "artifact": "probe",
                    "pid": self.running_identity().map(|record| record.pid),
                    "artifact_sha256": installed_hash(&self.install_root.join("probe")),
                    "running_sha256": self.running_identity().map(|record| record.sha256),
                    "ready": self.running_identity().is_some_and(|record| self.ready_for(record.pid, &record.sha256)),
                    "legacy_agent_running": legacy_agent_running(),
                }),
            ),
            "start" => self.start(&request),
            "stop" => self.stop(&request),
            "metrics" => self.metrics(&request),
            "diagnostics" => response(
                &request.id,
                "diagnostics",
                serde_json::json!({"probe_log":log_tail(&self.state_root.join("probe.log")),"agent_log":log_tail(&self.state_root.join("agent.log")),"main_status":fs::read("/tmp/mister-magik/main-status.json").ok().and_then(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).ok()),"running":self.running(),"running_sha256":self.running_identity().map(|p|p.sha256)}),
            ),
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
        let published_hash = installed_hash(&executable);
        if let Some(expected) = request
            .fields
            .get("expected_sha256")
            .and_then(serde_json::Value::as_str)
            && published_hash.as_deref() != Some(expected)
        {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"artifact-superseded","expected_sha256":expected,"published_sha256":published_hash}),
            );
        }
        if !restart
            && test_server.is_none()
            && self.running_identity().is_some_and(|record| {
                Some(&record.sha256) == published_hash.as_ref()
                    && self.ready_for(record.pid, &record.sha256)
            })
        {
            return response(
                &request.id,
                "started",
                serde_json::json!({"already_running":true,"ready":true,"sha256":published_hash}),
            );
        }
        if let Err(error) = fs::create_dir_all(&self.state_root) {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"state-directory","detail":error.to_string()}),
            );
        }
        let log = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.state_root.join("probe.log"))
        {
            Ok(log) => log,
            Err(error) => {
                return response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"probe-log","detail":error.to_string()}),
                );
            }
        };
        let stderr = match log.try_clone() {
            Ok(file) => file,
            Err(error) => {
                return response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"probe-log","detail":error.to_string()}),
                );
            }
        };
        if let Err(error) = fs::remove_file(self.readiness_path())
            && error.kind() != io::ErrorKind::NotFound
        {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"readiness-clear-failed","detail":error.to_string()}),
            );
        }
        if self.running()
            && let Err(error) = self.stop_owned_process()
        {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"stop-failed","detail":error}),
            );
        }
        if let Err(error) = main_handoff("mister_magik_suspend\n") {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"main-suspend-failed","detail":error,"recovery":main_handoff("mister_magik_resume\n").err()}),
            );
        }
        let mut command = Command::new(executable);
        command
            .env("MISTER_MAGIK2_STATE_ROOT", &self.state_root)
            .env(
                "MISTER_MAGIK2_ARTIFACT_SHA256",
                published_hash.as_deref().unwrap_or_default(),
            )
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr));
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
            Ok(child) => self.wait_for_readiness(
                request,
                child,
                published_hash.as_deref().unwrap_or_default(),
            ),
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
        if let Err(error) = self.stop_owned_process() {
            return response(
                &request.id,
                "error",
                serde_json::json!({"code":"stop-failed","detail":error}),
            );
        }
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

    fn upgrade_agent(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
        staged: upload::Staged,
    ) -> Result<(), FrameError> {
        let expected_hash = request
            .fields
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        staged
            .publish(&self.install_root.join("mister-magik2-agent"))
            .map_err(FrameError::Io)?;
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

    fn ready_for(&self, pid: u32, hash: &str) -> bool {
        fs::read(self.readiness_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|value| {
                value["pid"].as_u64() == Some(u64::from(pid))
                    && value["sha256"].as_str() == Some(hash)
                    && value["presentations"]
                        .as_u64()
                        .is_some_and(|count| count > 0)
            })
    }

    fn readiness_path(&self) -> PathBuf {
        self.state_root.join("probe-ready.json")
    }

    fn owned_process_path(&self) -> PathBuf {
        self.state_root.join("owned-process.json")
    }

    fn write_owned_process(&self, pid: u32, verified_hash: &str) -> Result<(), String> {
        let record = OwnedProcess {
            pid,
            executable: self.install_root.join("probe").display().to_string(),
            start_ticks: process_start_ticks(pid).ok_or("cannot identify child birth")?,
            sha256: verified_hash.to_owned(),
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

    fn running_identity(&self) -> Option<OwnedProcess> {
        if let Some(record) = self.read_owned_process()
            && !record.start_ticks.is_empty()
            && !record.sha256.is_empty()
            && process_start_ticks(record.pid).as_deref() == Some(&record.start_ticks)
            && fs::read_link(format!("/proc/{}/exe", record.pid))
                .ok()
                .is_some_and(|path| {
                    path.to_string_lossy().trim_end_matches(" (deleted)") == record.executable
                })
        {
            return Some(record);
        }
        // Older agents recorded only PID/path. Rediscover the actual executable
        // and persist its birth identity instead of treating it as a second app.
        let record = self.discover_owned_process()?;
        let _ = self.write_owned_process(record.pid, &record.sha256);
        Some(record)
    }

    fn owned_process_is_running(&self) -> bool {
        self.running_identity().is_some()
    }

    fn discover_owned_process(&self) -> Option<OwnedProcess> {
        let executable = self.install_root.join("probe").display().to_string();
        let mut matches = fs::read_dir("/proc").ok()?.flatten().filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            let path = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
            if path.to_string_lossy().trim_end_matches(" (deleted)") != executable {
                return None;
            }
            Some(OwnedProcess {
                pid,
                executable: executable.clone(),
                start_ticks: process_start_ticks(pid)?,
                sha256: installed_hash(&PathBuf::from(format!("/proc/{pid}/exe")))?,
            })
        });
        let record = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(record)
    }

    fn stop_owned_process(&self) -> Result<(), String> {
        let mut process = self.process.lock().expect("agent process state poisoned");
        if let Some(child) = process.as_mut() {
            if child.try_wait().map_err(|e| e.to_string())?.is_none() {
                child
                    .kill()
                    .map_err(|e| format!("cannot stop owned child: {e}"))?;
                let deadline = Instant::now() + Duration::from_secs(2);
                while child.try_wait().map_err(|e| e.to_string())?.is_none() {
                    if Instant::now() >= deadline {
                        return Err("owned child did not exit".into());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            *process = None;
            self.clear_owned_process();
            return Ok(());
        }
        drop(process);
        if let Some(record) = self.running_identity() {
            for signal in [libc::SIGTERM, libc::SIGKILL] {
                if process_start_ticks(record.pid).as_deref() != Some(&record.start_ticks) {
                    break;
                }
                // SAFETY: signal only the process with the verified birth identity.
                if unsafe { libc::kill(record.pid as libc::pid_t, signal) } != 0 {
                    return Err(format!(
                        "cannot stop adopted child: {}",
                        io::Error::last_os_error()
                    ));
                }
                let deadline = Instant::now() + Duration::from_secs(2);
                while self.owned_process_is_running() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if !self.owned_process_is_running() {
                    break;
                }
            }
            if self.owned_process_is_running() {
                return Err("adopted child did not exit".into());
            }
        }
        self.clear_owned_process();
        Ok(())
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
        let deadline = Instant::now() + TEST_SESSION_DEADLINE;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(FrameError::from)?;
        let endpoint = listener.local_addr().map_err(FrameError::from)?.to_string();
        let started = self.start_with_test_server(request, Some(endpoint));
        if started.op == "error" {
            // Startup may already have stopped the persistent probe. Attempt the
            // same restoration even when no relay was established.
            let mut restore = request.clone();
            restore.op = "start".into();
            restore.fields.remove("profile_id");
            restore
                .fields
                .insert("restart".into(), serde_json::json!(false));
            let restored = self.start(&restore);
            self.observation.record_log(format!(
                "test-session startup={:?} restore={:?}",
                started.fields, restored.fields
            ));
            let mut failed = started;
            failed.fields.insert(
                "persistent_restore".into(),
                serde_json::json!({"outcome":restored.op,"detail":restored.fields}),
            );
            return write_frame(stream, &failed, &[]);
        }
        let primary = (|| {
            write_frame(
                stream,
                &response(&request.id, "test-ready", serde_json::json!({"ready":true})),
                &[],
            )?;
            self.forward_test_connection(listener, stream, deadline)
        })();
        let cleanup = self.stop_owned_process();
        let mut restore = request.clone();
        restore.op = "start".into();
        restore.fields.remove("profile_id");
        restore
            .fields
            .insert("restart".into(), serde_json::json!(false));
        let restored = if cleanup.is_ok() {
            self.start(&restore)
        } else {
            response(
                &request.id,
                "error",
                serde_json::json!({"code":"session-stop-failed","detail":cleanup}),
            )
        };
        self.observation.record_log(format!(
            "test-session primary={primary:?} cleanup={cleanup:?} restore={:?}",
            restored.fields
        ));
        let recovery = if restored.op == "error" {
            Err(FrameError::Io(format!(
                "persistent restore: {:?}",
                restored.fields
            )))
        } else {
            Ok(())
        };
        primary.and(cleanup.map_err(FrameError::Io)).and(recovery)
    }

    fn forward_test_connection(
        &self,
        listener: TcpListener,
        stream: &mut TcpStream,
        deadline: Instant,
    ) -> Result<(), FrameError> {
        listener.set_nonblocking(true).map_err(FrameError::from)?;
        let connect_deadline = deadline.min(Instant::now() + TEST_APPLICATION_CONNECT_DEADLINE);
        let mut application = loop {
            match listener.accept() {
                Ok((connection, _)) => break connection,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && Instant::now() < connect_deadline =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Err(FrameError::Io(
                        "test application did not connect before deadline".to_owned(),
                    ));
                }
                Err(error) => {
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
            let _ = host_clone.shutdown(Shutdown::Both);
            let _ = device_to_host.shutdown(Shutdown::Both);
        });
        let primary = relay_until_deadline(stream, &mut application, deadline);
        let _ = application.shutdown(Shutdown::Both);
        let _ = stream.shutdown(Shutdown::Both);
        let _ = upstream.join();
        primary
    }

    fn wait_for_readiness(
        &self,
        request: &Envelope,
        mut child: Child,
        expected_hash: &str,
    ) -> Envelope {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if self.ready_for(child.id(), expected_hash) {
                // Hash the actual executable once after readiness, not on every
                // poll while the probe is starting and rendering its first frame.
                if installed_hash(&PathBuf::from(format!("/proc/{}/exe", child.id()))).as_deref()
                    != Some(expected_hash)
                {
                    return self.failed_start(
                        request,
                        "running-artifact-mismatch",
                        None,
                        &mut child,
                    );
                }
                if let Err(error) = self.write_owned_process(child.id(), expected_hash) {
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
        let deadline = Instant::now() + Duration::from_secs(2);
        let cleanup = loop {
            match child.try_wait() {
                Ok(Some(_)) => break None,
                Err(error) => break Some(error.to_string()),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Ok(None) => break Some("probe did not exit within cleanup deadline".to_owned()),
            }
        };
        let recovery = if cleanup.is_none() {
            main_handoff("mister_magik_resume\n")
        } else {
            Err(cleanup.clone().unwrap())
        };
        response(
            &request.id,
            "error",
            serde_json::json!({
                "code":code,
                "exit_status":status.and_then(|value| value.code()),
                "cleanup_error":cleanup,
                "recovery":recovery.err(),
            }),
        )
    }
}

fn log_tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map_or(0, |m| m.len());
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(16 * 1024)));
    let mut bytes = Vec::new();
    let _ = file.take(16 * 1024).read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
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

fn process_start_ticks(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field can contain spaces and parentheses; fields after its final
    // closing parenthesis start with state (field 3). Start time is field 22.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

fn main_handoff(command: &str) -> Result<(), String> {
    main_control::handoff(command)
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

#[cfg(test)]
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
    fn failed_test_start_retains_primary_and_restoration_outcomes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let root =
            std::env::temp_dir().join(format!("magik2-missing-probe-{}", std::process::id()));
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            Agent::with_state_root(
                "test".into(),
                "token".into(),
                root.clone(),
                root.join("state"),
            )
            .handle(&mut stream)
            .unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = Envelope {
            id: "failed-test".into(),
            op: "test-start".into(),
            token: "token".into(),
            fields: serde_json::Map::new(),
        };
        write_frame(&mut client, &request, &[]).unwrap();
        let (reply, _) = read_frame(&mut client).unwrap();
        assert_eq!(reply.fields["code"], "missing-application");
        assert_eq!(reply.fields["persistent_restore"]["outcome"], "error");
        assert_eq!(
            reply.fields["persistent_restore"]["detail"]["code"],
            "missing-application"
        );
        server.join().unwrap();
    }

    #[test]
    fn start_rejects_a_superseded_upload_before_touching_main() {
        let directory =
            std::env::temp_dir().join(format!("magik2-superseded-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("probe"), b"branch B").unwrap();
        let agent = Agent::with_state_root(
            "test".into(),
            "token".into(),
            directory.clone(),
            directory.join("state"),
        );
        let request = Envelope {
            id: "A-start".into(),
            op: "start".into(),
            token: "token".into(),
            fields: serde_json::from_value(serde_json::json!({"expected_sha256":"branch-A-hash"}))
                .unwrap(),
        };
        assert_eq!(agent.start(&request).fields["code"], "artifact-superseded");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn readiness_is_bound_to_process_and_artifact() {
        let directory =
            std::env::temp_dir().join(format!("magik2-readiness-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let agent = Agent::with_state_root(
            "test".into(),
            "token".into(),
            directory.clone(),
            directory.clone(),
        );
        fs::write(
            agent.readiness_path(),
            br#"{"pid":123,"sha256":"A","presentations":1}"#,
        )
        .unwrap();
        assert!(agent.ready_for(123, "A"));
        assert!(!agent.ready_for(124, "A"));
        assert!(!agent.ready_for(123, "B"));
        fs::remove_dir_all(directory).unwrap();
    }

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
        let request = Envelope {
            id: "partial".into(),
            op: "upload".into(),
            token: "token".into(),
            fields: serde_json::Map::new(),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request, b"payload").unwrap();
        bytes.pop();
        assert!(matches!(
            read_frame(&mut bytes.as_slice()),
            Err(FrameError::Io(_))
        ));
    }

    #[test]
    fn rejects_authentication_before_waiting_for_a_declared_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            Agent::new("test".into(), "correct".into(), std::env::temp_dir())
                .handle(&mut stream)
                .unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let header = br#"{"id":"bad","op":"upload","token":"incorrect"}"#;
        client
            .write_all(&(header.len() as u32).to_be_bytes())
            .unwrap();
        client
            .write_all(&(32_u64 * 1024 * 1024).to_be_bytes())
            .unwrap();
        client.write_all(header).unwrap(); // deliberately send no body
        assert_eq!(
            read_frame(&mut client).unwrap().0.fields["code"],
            "authentication-failed"
        );
        server.join().unwrap();
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
