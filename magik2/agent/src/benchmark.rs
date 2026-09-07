//! One bounded process group; no framebuffer readiness or UI test bridge.
use super::*;
use std::os::fd::AsRawFd;

const STDOUT_LIMIT: usize = 256 * 1024;
const STDERR_LIMIT: usize = 8 * 1024;

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        // SAFETY: the child is leader of its own process group; never signal our group.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        while self.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: pipe owns a live descriptor. Preserve existing descriptor flags.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<bool, String> {
    let mut buffer = [0; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                let remaining = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..n.min(remaining)]);
                if n > remaining {
                    return Err("benchmark output limit exceeded".into());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[derive(Default)]
struct Output {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    error: Option<String>,
}

fn execute(command: &mut Command, timeout: Duration, cancelled: impl Fn() -> bool) -> Output {
    let mut output = Output::default();
    let result = (|| -> Result<(), String> {
        let mut child = Running(
            command
                .process_group(0)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?,
        );
        let mut stdout = child.0.stdout.take().ok_or("missing stdout")?;
        let mut stderr = child.0.stderr.take().ok_or("missing stderr")?;
        nonblocking(&stdout).map_err(|e| e.to_string())?;
        nonblocking(&stderr).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let mut status = None;
        loop {
            if cancelled() {
                return Err("benchmark cancelled: client disconnected".into());
            }
            if Instant::now() >= deadline {
                return Err("benchmark deadline exceeded".into());
            }
            let out_done = drain(&mut stdout, &mut output.stdout, STDOUT_LIMIT)?;
            let err_done = drain(&mut stderr, &mut output.stderr, STDERR_LIMIT)?;
            if status.is_none() {
                status = child.0.try_wait().map_err(|e| e.to_string())?;
            }
            if let Some(status) = status {
                output.code = status.code();
                if out_done && err_done {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    output.error = result.err();
    output
}

fn workload_valid(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.len() <= 48
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Agent {
    pub(super) fn run_benchmark(
        &self,
        stream: &mut TcpStream,
        request: &Envelope,
        body: &[u8],
    ) -> Result<(), FrameError> {
        let workload = request
            .fields
            .get("workload")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let mode = request
            .fields
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let hash = installed_hash(&self.install_root.join("mini-magik"));
        if !body.is_empty()
            || !workload_valid(workload)
            || !matches!(mode, "timing" | "visual" | "pmu-neon" | "pmu-memory")
            || hash.is_none()
            || request
                .fields
                .get("expected_sha256")
                .and_then(serde_json::Value::as_str)
                != hash.as_deref()
        {
            return write_frame(
                stream,
                &response(
                    &request.id,
                    "error",
                    serde_json::json!({"code":"invalid-benchmark-request"}),
                ),
                &[],
            );
        }
        let prepared = self
            .stop_owned_process()
            .and_then(|()| main_handoff("mister_magik_suspend\n"));
        let output = match prepared {
            Ok(()) => {
                let mut command = Command::new(self.install_root.join("mini-magik"));
                command.args(["--bench", workload, "--mode", mode]);
                command.env(
                    "MISTER_MAGIK2_ARTIFACT_SHA256",
                    hash.as_deref().unwrap_or_default(),
                );
                execute(
                    &mut command,
                    Duration::from_secs(if mode == "visual" { 90 } else { 30 }),
                    || {
                        let mut byte = 0_u8;
                        // SAFETY: the socket and one-byte buffer are live; this never consumes data.
                        unsafe {
                            libc::recv(
                                stream.as_raw_fd(),
                                (&raw mut byte).cast(),
                                1,
                                libc::MSG_PEEK | libc::MSG_DONTWAIT,
                            ) == 0
                        }
                    },
                )
            }
            Err(error) => Output {
                error: Some(error),
                ..Output::default()
            },
        };
        let recovery = main_handoff("mister_magik_resume\n");
        write_frame(
            stream,
            &response(
                &request.id,
                "benchmark-complete",
                serde_json::json!({
                    "exit_code":output.code,"sha256":hash,"stderr":String::from_utf8_lossy(&output.stderr),
                    "error":output.error,"launcher_resumed":recovery.is_ok(),"recovery":recovery.err()
                }),
            ),
            &output.stdout,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", script]);
        c
    }
    #[test]
    fn collects_exit_and_output() {
        let r = execute(
            &mut shell("printf ok; printf bad >&2; exit 7"),
            Duration::from_secs(1),
            || false,
        );
        assert_eq!(r.code, Some(7));
        assert_eq!(r.stdout, b"ok");
        assert_eq!(r.stderr, b"bad");
        assert!(r.error.is_none());
    }
    #[test]
    fn bounds_inherited_pipes_and_cancellation() {
        let start = Instant::now();
        let r = execute(
            &mut shell("sleep 20 & exit 0"),
            Duration::from_millis(50),
            || false,
        );
        assert!(r.error.unwrap().contains("deadline"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let r = execute(&mut shell("sleep 20"), Duration::from_secs(1), || true);
        assert!(r.error.unwrap().contains("cancelled"));
    }
    #[test]
    fn limits_output_and_identifiers() {
        let r = execute(
            &mut shell(
                "while :; do printf '01234567890123456789012345678901234567890123456789' >&2; done",
            ),
            Duration::from_secs(2),
            || false,
        );
        assert!(r.error.unwrap().contains("output limit"));
        assert_eq!(r.stderr.len(), STDERR_LIMIT);
        assert!(workload_valid("blend-next"));
        assert!(!workload_valid("../x"));
        assert!(!workload_valid("--shell"));
    }
}
