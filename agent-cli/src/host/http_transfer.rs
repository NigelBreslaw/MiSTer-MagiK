// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::Result;
use super::remote::shell_quote;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ACCEPT_POLL: Duration = Duration::from_millis(10);
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(2);
const RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(120);
const SERVE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const MAX_REJECTED_REQUESTS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HttpServeReport {
    pub(super) bytes: u64,
}

#[derive(Debug)]
enum ServerOutcome {
    Served(HttpServeReport),
    Cancelled,
}

pub(super) struct OneShotHttpArtifactServer {
    url: String,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<std::result::Result<ServerOutcome, String>>>,
}

impl OneShotHttpArtifactServer {
    pub(super) fn start(remote_host: &str, source: &Path) -> Result<Self> {
        let bind_ip = route_local_ip(remote_host)?;
        let token = random_token()?;
        Self::start_bound(source, bind_ip, &token, SERVE_TIMEOUT)
    }

    pub(super) fn url(&self) -> &str {
        &self.url
    }

    pub(super) fn finish(mut self) -> Result<HttpServeReport> {
        match self.join()? {
            ServerOutcome::Served(report) => Ok(report),
            ServerOutcome::Cancelled => Err("delivery HTTP server was cancelled".into()),
        }
    }

    pub(super) fn cancel(mut self) -> Result<()> {
        self.cancel.store(true, Ordering::Release);
        match self.join()? {
            ServerOutcome::Served(_) | ServerOutcome::Cancelled => Ok(()),
        }
    }

    pub(super) fn start_bound(
        source: &Path,
        bind_ip: IpAddr,
        token: &str,
        timeout: Duration,
    ) -> Result<Self> {
        let metadata = fs::metadata(source)?;
        if !metadata.is_file() {
            return Err(format!("delivery HTTP source is not a file: {}", source.display()).into());
        }
        let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let host = match address.ip() {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        let request_path = format!("/{token}");
        let url = format!("http://{host}:{}{request_path}", address.port());
        let source = source.to_path_buf();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker = thread::spawn(move || {
            serve_one(
                listener,
                &source,
                metadata.len(),
                &request_path,
                timeout,
                &worker_cancel,
            )
            .map_err(|error| error.to_string())
        });
        Ok(Self {
            url,
            cancel,
            worker: Some(worker),
        })
    }

    fn join(&mut self) -> Result<ServerOutcome> {
        let result = self
            .worker
            .take()
            .ok_or("delivery HTTP server was already joined")?
            .join()
            .map_err(|_| "delivery HTTP server thread panicked")?;
        result.map_err(Into::into)
    }
}

impl Drop for OneShotHttpArtifactServer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn curl_fetch_command(url: &str, destination: &str, expected_bytes: u64) -> String {
    format!(
        "set -eu; rm -f {destination}; command -v curl >/dev/null 2>&1; curl --fail --silent --show-error --proto '=http' --proto-redir '=http' --max-redirs 0 --connect-timeout 5 --max-time 120 --max-filesize {expected_bytes} --header 'Accept-Encoding: identity' --output {destination} {url}; test \"$(wc -c < {destination})\" -eq {expected_bytes}",
        destination = shell_quote(destination),
        url = shell_quote(url),
    )
}

fn route_local_ip(remote_host: &str) -> Result<IpAddr> {
    let remote = format!("{remote_host}:22")
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer route for delivery HTTP")?;
    let bind = match remote.ip() {
        IpAddr::V4(_) => "0.0.0.0:0",
        IpAddr::V6(_) => "[::]:0",
    };
    let socket = UdpSocket::bind(bind)?;
    socket.connect(remote)?;
    Ok(socket.local_addr()?.ip())
}

fn random_token() -> Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn serve_one(
    listener: TcpListener,
    source: &Path,
    source_bytes: u64,
    request_path: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<ServerOutcome> {
    let deadline = Instant::now() + timeout;
    let mut rejected = 0;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(ServerOutcome::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err("delivery HTTP server timed out waiting for the MiSTer".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                match handle_request(&mut stream, source, source_bytes, request_path)? {
                    Some(report) => return Ok(ServerOutcome::Served(report)),
                    None => {
                        rejected += 1;
                        if rejected >= MAX_REJECTED_REQUESTS {
                            return Err("delivery HTTP server rejected too many requests".into());
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn handle_request(
    stream: &mut TcpStream,
    source: &Path,
    source_bytes: u64,
    request_path: &str,
) -> Result<Option<HttpServeReport>> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(REQUEST_READ_TIMEOUT))?;
    stream.set_write_timeout(Some(RESPONSE_WRITE_TIMEOUT))?;
    let request = read_request(stream)?;
    let mut lines = request.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_ascii_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    let has_range = lines.any(|line| line.to_ascii_lowercase().starts_with("range:"));
    if method != "GET" {
        write_status(stream, "405 Method Not Allowed")?;
        return Ok(None);
    }
    if path != request_path || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        write_status(stream, "404 Not Found")?;
        return Ok(None);
    }
    if has_range {
        write_status(stream, "400 Bad Request")?;
        return Ok(None);
    }

    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {source_bytes}\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nAccept-Ranges: none\r\nConnection: close\r\n\r\n"
    )?;
    let mut file = File::open(source)?;
    let copied = std::io::copy(&mut file, stream)?;
    stream.flush()?;
    if copied != source_bytes {
        return Err(format!(
            "delivery HTTP source changed while serving expected={source_bytes} actual={copied}"
        )
        .into());
    }
    Ok(Some(HttpServeReport { bytes: copied }))
}

fn read_request(stream: &mut TcpStream) -> Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while request.len() < MAX_REQUEST_BYTES {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    if request.len() >= MAX_REQUEST_BYTES || !request.windows(4).any(|window| window == b"\r\n\r\n")
    {
        return Err("delivery HTTP request headers are invalid or oversized".into());
    }
    String::from_utf8(request).map_err(|_| "delivery HTTP request is not UTF-8".into())
}

fn write_status(stream: &mut TcpStream, status: &str) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(contents: &[u8]) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mister-magik-http-transfer-{}-{stamp}",
            std::process::id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    fn request(url: &str, method: &str, path: &str, headers: &str) -> Vec<u8> {
        let authority = url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let mut stream = TcpStream::connect(authority).unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: test\r\n{headers}\r\n"
        )
        .unwrap();
        stream.flush().unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        response
    }

    #[test]
    fn serves_exact_artifact_once() {
        let path = temp_file(b"exact-runtime-binary");
        let server = OneShotHttpArtifactServer::start_bound(
            &path,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "token",
            Duration::from_secs(1),
        )
        .unwrap();
        let response = request(server.url(), "GET", "/token", "");
        assert!(response.ends_with(b"exact-runtime-binary"));
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert_eq!(server.finish().unwrap().bytes, 20);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_other_methods_paths_and_ranges_before_serving() {
        let path = temp_file(b"binary");
        let server = OneShotHttpArtifactServer::start_bound(
            &path,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "token",
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(request(server.url(), "POST", "/token", "").starts_with(b"HTTP/1.1 405"));
        assert!(request(server.url(), "GET", "/wrong", "").starts_with(b"HTTP/1.1 404"));
        assert!(
            request(server.url(), "GET", "/token", "Range: bytes=0-1\r\n")
                .starts_with(b"HTTP/1.1 400")
        );
        assert!(request(server.url(), "GET", "/token", "").starts_with(b"HTTP/1.1 200"));
        assert_eq!(server.finish().unwrap().bytes, 6);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn cancellation_stops_waiting_server() {
        let path = temp_file(b"binary");
        let server = OneShotHttpArtifactServer::start_bound(
            &path,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "token",
            Duration::from_secs(1),
        )
        .unwrap();
        server.cancel().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn waiting_server_has_a_bounded_deadline() {
        let path = temp_file(b"binary");
        let server = OneShotHttpArtifactServer::start_bound(
            &path,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "token",
            Duration::from_millis(25),
        )
        .unwrap();
        assert!(
            server
                .finish()
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn curl_command_is_raw_bounded_and_shell_quoted() {
        let command = curl_fetch_command(
            "http://192.0.2.1:1234/a'b",
            "/tmp/runtime upload",
            24_478_392,
        );
        assert!(command.contains("--proto '=http'"));
        assert!(command.contains("--max-redirs 0"));
        assert!(command.contains("--max-filesize 24478392"));
        assert!(command.contains("Accept-Encoding: identity"));
        assert!(command.contains("'http://192.0.2.1:1234/a'\"'\"'b'"));
        assert!(command.contains("'/tmp/runtime upload'"));
        assert!(!command.contains("gzip"));
    }
}
