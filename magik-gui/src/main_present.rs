use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_PRESENT_REQUEST_PATH: &str = "/tmp/mister-magik/present-request-v1";
pub const DEFAULT_PRESENT_ACK_PATH: &str = "/tmp/mister-magik/present-ack-v1";
const DEFAULT_PRESENT_ACK_TIMEOUT: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MainPresentRequest {
    pub sequence: u32,
    pub buffer_index: u8,
    pub width: usize,
    pub height: usize,
    pub stride_bytes: usize,
}

impl MainPresentRequest {
    pub fn encode(self) -> String {
        format!(
            "mister_magik_present_vsync_v1 sequence={} buffer={} width={} height={} stride={}\n",
            self.sequence, self.buffer_index, self.width, self.height, self.stride_bytes
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MainPresentAck {
    pub sequence: u32,
    pub status: String,
    pub buffer_index: u8,
    pub wait_us: u64,
    pub route_us: u64,
}

impl MainPresentAck {
    pub fn ok(&self) -> bool {
        self.status == "ok"
    }
}

#[derive(Debug)]
pub enum MainPresentError {
    Io(io::Error),
    AckTimeout { sequence: u32 },
    AckParse(String),
    AckSequenceMismatch { expected: u32, actual: u32 },
    Rejected(MainPresentAck),
}

impl std::fmt::Display for MainPresentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "main present I/O failed: {e}"),
            Self::AckTimeout { sequence } => {
                write!(f, "main present ack timed out for sequence {sequence}")
            }
            Self::AckParse(e) => write!(f, "main present ack parse failed: {e}"),
            Self::AckSequenceMismatch { expected, actual } => {
                write!(
                    f,
                    "main present ack sequence mismatch expected {expected} got {actual}"
                )
            }
            Self::Rejected(ack) => write!(
                f,
                "main present rejected sequence {} status={}",
                ack.sequence, ack.status
            ),
        }
    }
}

impl std::error::Error for MainPresentError {}

impl From<io::Error> for MainPresentError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct MainPresentClient {
    request_path: PathBuf,
    ack_path: PathBuf,
    timeout: Duration,
}

impl MainPresentClient {
    pub fn default_paths() -> Self {
        Self {
            request_path: PathBuf::from(DEFAULT_PRESENT_REQUEST_PATH),
            ack_path: PathBuf::from(DEFAULT_PRESENT_ACK_PATH),
            timeout: DEFAULT_PRESENT_ACK_TIMEOUT,
        }
    }

    pub fn present(&self, request: MainPresentRequest) -> Result<MainPresentAck, MainPresentError> {
        let _ = fs::remove_file(&self.ack_path);
        write_request_atomically(&self.request_path, &request.encode())?;
        let deadline = Instant::now() + self.timeout;
        let mut last_parse_error = None;
        loop {
            match fs::read_to_string(&self.ack_path) {
                Ok(text) => {
                    let ack = match parse_ack(&text) {
                        Ok(ack) => ack,
                        Err(e) => {
                            last_parse_error = Some(e);
                            if Instant::now() >= deadline {
                                return Err(MainPresentError::AckParse(
                                    last_parse_error.unwrap_or_else(|| {
                                        "ack parse failed before timeout".to_string()
                                    }),
                                ));
                            }
                            thread::sleep(Duration::from_micros(250));
                            continue;
                        }
                    };
                    if ack.sequence != request.sequence {
                        return Err(MainPresentError::AckSequenceMismatch {
                            expected: request.sequence,
                            actual: ack.sequence,
                        });
                    }
                    if ack.ok() {
                        return Ok(ack);
                    }
                    return Err(MainPresentError::Rejected(ack));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(MainPresentError::Io(e)),
            }
            if Instant::now() >= deadline {
                if let Some(error) = last_parse_error {
                    return Err(MainPresentError::AckParse(error));
                }
                return Err(MainPresentError::AckTimeout {
                    sequence: request.sequence,
                });
            }
            thread::sleep(Duration::from_micros(250));
        }
    }
}

fn write_request_atomically(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(tmp, path)
}

pub fn parse_ack(line: &str) -> Result<MainPresentAck, String> {
    let mut fields = line.split_whitespace();
    match fields.next() {
        Some("mister_magik_present_ack_v1") => {}
        _ => return Err("bad ack prefix".to_string()),
    }
    let mut sequence = None;
    let mut status = None;
    let mut buffer_index = None;
    let mut wait_us = None;
    let mut route_us = None;
    for field in fields {
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| format!("bad ack field {field}"))?;
        match key {
            "sequence" => sequence = Some(parse_field(value, "sequence")?),
            "status" => status = Some(value.to_string()),
            "buffer" => buffer_index = Some(parse_field(value, "buffer")?),
            "wait_us" => wait_us = Some(parse_field(value, "wait_us")?),
            "route_us" => route_us = Some(parse_field(value, "route_us")?),
            _ => return Err(format!("unknown ack field {key}")),
        }
    }
    Ok(MainPresentAck {
        sequence: sequence.ok_or("missing sequence")?,
        status: status.ok_or("missing status")?,
        buffer_index: buffer_index.ok_or("missing buffer")?,
        wait_us: wait_us.ok_or("missing wait_us")?,
        route_us: route_us.ok_or("missing route_us")?,
    })
}

fn parse_field<T: std::str::FromStr>(value: &str, label: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("invalid {label} value {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_request_encodes_main_request_line() {
        assert_eq!(
            MainPresentRequest {
                sequence: 7,
                buffer_index: 2,
                width: 960,
                height: 540,
                stride_bytes: 1920,
            }
            .encode(),
            "mister_magik_present_vsync_v1 sequence=7 buffer=2 width=960 height=540 stride=1920\n"
        );
    }

    #[test]
    fn parses_present_ack() {
        let ack = parse_ack(
            "mister_magik_present_ack_v1 sequence=7 status=ok buffer=2 wait_us=16000 route_us=22\n",
        )
        .unwrap();

        assert_eq!(ack.sequence, 7);
        assert!(ack.ok());
        assert_eq!(ack.buffer_index, 2);
        assert_eq!(ack.wait_us, 16000);
        assert_eq!(ack.route_us, 22);
    }

    #[test]
    fn rejects_bad_present_ack() {
        assert!(parse_ack("wrong sequence=7 status=ok buffer=2 wait_us=1 route_us=2").is_err());
        assert!(
            parse_ack("mister_magik_present_ack_v1 sequence=7 status=ok buffer=2 wait_us=1")
                .is_err()
        );
        assert!(parse_ack(
            "mister_magik_present_ack_v1 sequence=7 status=ok buffer=x wait_us=1 route_us=2"
        )
        .is_err());
    }

    #[test]
    fn present_retries_transient_partial_ack_until_complete() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-main-present-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let request_path = root.join("present-request-v1");
        let ack_path = root.join("present-ack-v1");
        let ack_path_for_thread = ack_path.clone();
        let request_path_for_thread = request_path.clone();
        let writer = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            while !request_path_for_thread.exists() {
                assert!(Instant::now() < deadline, "request was not written");
                thread::sleep(Duration::from_micros(250));
            }
            fs::write(&ack_path_for_thread, "mister_magik_present_ack_v1").unwrap();
            thread::sleep(Duration::from_millis(2));
            fs::write(
                &ack_path_for_thread,
                "mister_magik_present_ack_v1 sequence=9 status=ok buffer=1 wait_us=10 route_us=2\n",
            )
            .unwrap();
        });
        let client = MainPresentClient {
            request_path,
            ack_path,
            timeout: Duration::from_millis(50),
        };

        let ack = client
            .present(MainPresentRequest {
                sequence: 9,
                buffer_index: 1,
                width: 960,
                height: 540,
                stride_bytes: 1920,
            })
            .unwrap();

        writer.join().unwrap();
        assert_eq!(ack.sequence, 9);
        assert_eq!(ack.buffer_index, 1);
        let _ = fs::remove_dir_all(root);
    }
}
