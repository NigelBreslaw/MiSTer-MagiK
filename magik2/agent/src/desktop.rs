//! Read-only Desktop API on the existing authenticated native connection.
use crate::{
    Envelope, response, sd, telemetry,
    wire::{self, FrameError},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub const OPERATIONS: &[&str] = &[
    "dashboard-status",
    "sd-list",
    "sd-stat",
    "sd-preview",
    "sd-mra",
    "framebuffer-stream",
    "telemetry-stream",
];
static FRAME: Mutex<()> = Mutex::new(());
static TELEMETRY: Mutex<()> = Mutex::new(());
const LEASE: &str = "/tmp/mister-magik/realtime-frame-analytics";

fn dashboard() -> Value {
    let processes = telemetry::processes(Path::new("/proc"));
    let (status, current) = telemetry::current_status(&processes);
    let main = telemetry::read_json("/tmp/mister-magik/main-status.json");
    let text = |path: &str| fs::read_to_string(path).ok().map(|s| s.trim().to_string());
    json!({"processes":processes,"network":{"mac":text("/sys/class/net/eth0/address"),"carrier":text("/sys/class/net/eth0/carrier"),"operstate":text("/sys/class/net/eth0/operstate")},"files":{"main_status":main,"slint_status":status,"slint_status_current":current}})
}

pub fn handle(stream: &mut TcpStream, request: &Envelope, body: &[u8]) -> Result<(), FrameError> {
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    if !body.is_empty() {
        return error(
            stream,
            request,
            "invalid-request",
            "Desktop operations take no body",
        );
    }
    let sd_operation = request.op.starts_with("sd-");
    if request
        .fields
        .iter()
        .any(|(key, value)| match key.as_str() {
            "path" => !sd_operation || !value.is_string(),
            "show_hidden" => request.op != "sd-list" || !value.is_boolean(),
            _ => true,
        })
    {
        return error(
            stream,
            request,
            "invalid-request",
            "unsupported Desktop request fields",
        );
    }
    match request.op.as_str() {
        "framebuffer-stream" => framebuffer(stream, request),
        "telemetry-stream" => telemetry_stream(stream, request),
        op => {
            let path = request
                .fields
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("/");
            let root = Path::new("/media/fat");
            let result = match op {
                "dashboard-status" => Ok((dashboard(), Vec::new())),
                "sd-list" => sd::list_dir_fast_at_root(
                    root,
                    path,
                    request
                        .fields
                        .get("show_hidden")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
                .map(|v| (v, Vec::new())),
                "sd-stat" => sd::stat_item_at_root(root, path).map(|v| (v, Vec::new())),
                "sd-mra" => sd::parse_mra_at_root(root, path).map(|v| (v, Vec::new())),
                "sd-preview" => {
                    sd::preview_image_at_root(root, path).map(|v| (v.result, v.payload))
                }
                _ => unreachable!(),
            };
            match result {
                Ok((value, payload)) if op == "sd-preview" => wire::write_frame(
                    stream,
                    &response(&request.id, "sd-preview-result", value),
                    &payload,
                ),
                Ok((value, _)) => {
                    let bytes = serde_json::to_vec(&value).expect("JSON");
                    if bytes.len() > wire::MAX_BODY_BYTES {
                        return error(
                            stream,
                            request,
                            "result-too-large",
                            "result exceeds native body limit",
                        );
                    }
                    wire::write_frame(
                        stream,
                        &response(
                            &request.id,
                            &format!("{op}-result"),
                            json!({"format":"json"}),
                        ),
                        &bytes,
                    )
                }
                Err(detail) => error(stream, request, "sd-unavailable", &detail),
            }
        }
    }
}
fn error(
    stream: &mut TcpStream,
    request: &Envelope,
    code: &str,
    detail: &str,
) -> Result<(), FrameError> {
    wire::write_frame(
        stream,
        &response(&request.id, "error", json!({"code":code,"detail":detail})),
        &[],
    )
}
fn framebuffer(stream: &mut TcpStream, request: &Envelope) -> Result<(), FrameError> {
    let Ok(_owner) = FRAME.try_lock() else {
        return error(
            stream,
            request,
            "stream-busy",
            "framebuffer already has a native consumer",
        );
    };
    let producer = match TcpStream::connect_timeout(
        &"127.0.0.1:7499".parse().unwrap(),
        Duration::from_secs(2),
    ) {
        Ok(p) => p,
        Err(e) => return error(stream, request, "producer-unavailable", &e.to_string()),
    };
    relay(stream, producer, request)
}
fn relay(
    stream: &mut TcpStream,
    mut producer: TcpStream,
    request: &Envelope,
) -> Result<(), FrameError> {
    producer.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    wire::write_frame(
        stream,
        &response(
            &request.id,
            "framebuffer-stream-ready",
            json!({"source":"producer-pre-ownership-transfer","encoding":"lz4-block-size-prepended","format":"rgb565-le"}),
        ),
        &[],
    )?;
    let mut cancel = stream.try_clone()?;
    cancel.set_read_timeout(Some(Duration::from_millis(250)))?;
    let shutdown = producer.try_clone()?;
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut byte = [0];
            while !done.load(Ordering::Relaxed) {
                match cancel.read(&mut byte) {
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    _ => break,
                }
            }
            done.store(true, Ordering::Relaxed);
            let _ = shutdown.shutdown(Shutdown::Both);
        });
        let result = (|| {
            let mut bytes = [0; 64 * 1024];
            while !done.load(Ordering::Relaxed) {
                match producer.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => stream.write_all(&bytes[..n])?,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        })();
        done.store(true, Ordering::Relaxed);
        let _ = producer.shutdown(Shutdown::Both);
        let _ = stream.shutdown(Shutdown::Both);
        result
    })
}

struct Lease {
    path: std::path::PathBuf,
    last: Option<std::time::SystemTime>,
}
impl Lease {
    fn refresh(&mut self) -> std::io::Result<()> {
        let path = &self.path;
        fs::create_dir_all(path.parent().unwrap())?;
        let temporary = path.with_extension(format!("native-{}", std::process::id()));
        fs::write(&temporary, b"process\n")?;
        fs::rename(&temporary, path)?;
        self.last = fs::metadata(path)?.modified().ok();
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.last.is_some()
            && fs::metadata(&self.path).and_then(|m| m.modified()).ok() == self.last
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn telemetry_stream(stream: &mut TcpStream, request: &Envelope) -> Result<(), FrameError> {
    let Ok(_owner) = TELEMETRY.try_lock() else {
        return error(
            stream,
            request,
            "stream-busy",
            "telemetry already has a native consumer",
        );
    };
    wire::write_frame(
        stream,
        &response(
            &request.id,
            "telemetry-stream-ready",
            json!({"cadence_ms":1000}),
        ),
        &[],
    )?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let mut sampler = telemetry::Sampler::default();
    let mut lease = Lease {
        path: LEASE.into(),
        last: None,
    };
    for seq in 0..u64::MAX {
        lease.refresh()?;
        let bytes = serde_json::to_vec(&sampler.sample(seq)).expect("JSON");
        wire::write_frame(
            stream,
            &response(&request.id, "telemetry-sample", json!({"format":"json"})),
            &bytes,
        )?;
        std::thread::sleep(Duration::from_millis(1000));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, sync::mpsc, time::Instant};
    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let a = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let b = listener.accept().unwrap().0;
        (a, b)
    }
    #[test]
    fn lease_cleanup_preserves_a_replaced_owner() {
        let root = std::env::temp_dir().join(format!("native-lease-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("lease");
        let mut lease = Lease {
            path: path.clone(),
            last: None,
        };
        lease.refresh().unwrap();
        drop(lease);
        assert!(!path.exists());
        let mut lease = Lease {
            path: path.clone(),
            last: None,
        };
        lease.refresh().unwrap();
        // A changed marker belongs to another producer and must survive cleanup.
        lease.last = Some(std::time::UNIX_EPOCH);
        drop(lease);
        assert!(path.exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn relay_cancellation_closes_an_idle_producer_promptly() {
        let (mut consumer, mut server) = pair();
        let (producer, mut source) = pair();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let request = Envelope {
                id: "fixture".into(),
                op: "framebuffer-stream".into(),
                token: String::new(),
                fields: Default::default(),
            };
            let result = relay(&mut server, producer, &request);
            done_tx.send(result).unwrap();
        });
        let (ready, body) = wire::read_frame(&mut consumer).unwrap();
        assert_eq!(ready.op, "framebuffer-stream-ready");
        assert!(body.is_empty());
        source.write_all(b"producer bytes").unwrap();
        let mut bytes = [0; 14];
        consumer.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"producer bytes");
        let started = Instant::now();
        consumer.shutdown(Shutdown::Both).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        source
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        assert_eq!(source.read(&mut [0]).unwrap(), 0);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
