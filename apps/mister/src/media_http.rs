// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_media_contract::{
    MAX_MANIFEST_BYTES, MAX_MANIFEST_SIGNATURE_BYTES, ManifestTrustMode,
    configured_manifest_trust_mode, manifest_signature_url, validate_https_manifest_url,
    verify_manifest_signature,
};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const MANIFEST_CONNECT_TIMEOUT_SECS: u64 = 10;
const MANIFEST_FETCH_TIMEOUT_SECS: u64 = 15;

/// Drain the entire pipe concurrently, but retain at most 2 KiB. Retaining
/// only a prefix without draining the remainder can deadlock curl on stderr.
pub(crate) fn drain_curl_stderr(
    mut pipe: impl Read + Send + 'static,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut retained = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let keep = count.min(2048usize.saturating_sub(retained.len()));
                    retained.extend_from_slice(&buffer[..keep]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        crate::media_diagnostics::sanitize(&String::from_utf8_lossy(&retained))
    })
}

#[derive(Debug)]
pub struct ManifestFetch {
    pub bytes: Vec<u8>,
    pub headers: String,
}

pub fn fetch_manifest(url: &str) -> Result<ManifestFetch, String> {
    let result = fetch_manifest_with(
        url,
        configured_manifest_trust_mode(),
        fetch_https_bytes,
        verify_manifest_signature,
    );
    match &result {
        Ok(manifest) => crate::media_diagnostics::record(
            "manifest_fetched",
            format!("url={url} bytes={}", manifest.bytes.len()),
            false,
        ),
        Err(error) => crate::media_diagnostics::record(
            "manifest_failed",
            format!("url={url} stage=fetch_or_trust detail={error}"),
            true,
        ),
    }
    result
}

fn fetch_manifest_with<F, V>(
    url: &str,
    trust_mode: ManifestTrustMode,
    mut fetch: F,
    mut verify: V,
) -> Result<ManifestFetch, String>
where
    F: FnMut(&str, u64, &str) -> Result<HttpsFetch, String>,
    V: FnMut(&[u8], &[u8]) -> Result<String, String>,
{
    validate_https_manifest_url(url)?;
    let manifest = fetch(url, MAX_MANIFEST_BYTES, "manifest")?;
    if trust_mode == ManifestTrustMode::SignedHttps {
        let signature_url = manifest_signature_url(url)?;
        let signature = fetch(
            &signature_url,
            MAX_MANIFEST_SIGNATURE_BYTES,
            "manifest signature",
        )?;
        verify(&manifest.bytes, &signature.bytes)?;
    }
    Ok(ManifestFetch {
        bytes: manifest.bytes,
        headers: manifest.headers,
    })
}

pub fn write_bounded_stream_chunk(
    output: &mut impl Write,
    hash: &mut impl Write,
    chunk: &[u8],
    bytes: u64,
    expected_bytes: u64,
    object_label: &str,
) -> Result<u64, String> {
    let remaining = expected_bytes.saturating_sub(bytes);
    let allowed = remaining.min(chunk.len() as u64) as usize;
    output
        .write_all(&chunk[..allowed])
        .map_err(|error| format!("write streamed {object_label}: {error}"))?;
    hash.write_all(&chunk[..allowed])
        .map_err(|error| format!("write {object_label} hash stream: {error}"))?;
    let written = bytes.saturating_add(allowed as u64);
    if allowed != chunk.len() {
        return Err(format!(
            "{object_label} exceeds declared size expected={expected_bytes}"
        ));
    }
    Ok(written)
}

struct HttpsFetch {
    bytes: Vec<u8>,
    headers: String,
}

fn fetch_https_bytes(url: &str, max_bytes: u64, label: &str) -> Result<HttpsFetch, String> {
    validate_https_manifest_url(url)?;
    let headers_path = temporary_headers_path(label);
    let mut command = Command::new("curl");
    add_https_fetch_args(&mut command, url, &headers_path, max_bytes);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn curl for {label}: {error}"))?;
    let stderr = child.stderr.take().map(drain_curl_stderr);
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_child(&mut child);
            let _ = fs::remove_file(&headers_path);
            return Err(format!("missing curl stdout for {label}"));
        }
    };
    let mut bytes = Vec::new();
    let read_result = stdout
        .by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {label}: {error}"));
    if read_result.is_err() || bytes.len() as u64 > max_bytes {
        terminate_child(&mut child);
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for {label} curl: {error}"))?;
    let headers = fs::read_to_string(&headers_path).unwrap_or_default();
    let _ = fs::remove_file(headers_path);
    read_result?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!("{label} exceeds {max_bytes} bytes"));
    }
    if !output.status.success() {
        let stderr = stderr
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        if stderr.is_empty() {
            return Err(format!("{label} curl exited with {}", output.status));
        }
        return Err(format!(
            "{label} curl exited with {}: {stderr}",
            output.status
        ));
    }
    Ok(HttpsFetch { bytes, headers })
}

fn add_https_fetch_args(command: &mut Command, url: &str, headers_path: &Path, max_bytes: u64) {
    command
        .arg("--fail")
        .arg("--silent")
        .arg("--show-error")
        .arg("--location")
        .arg("--proto")
        .arg("=https")
        .arg("--proto-redir")
        .arg("=https")
        .arg("--connect-timeout")
        .arg(MANIFEST_CONNECT_TIMEOUT_SECS.to_string())
        .arg("--max-time")
        .arg(MANIFEST_FETCH_TIMEOUT_SECS.to_string())
        .arg("--max-filesize")
        .arg(max_bytes.to_string())
        .arg("--header")
        .arg("Accept-Encoding: identity")
        .arg("-D")
        .arg(headers_path)
        .arg("-o")
        .arg("-");
    if Path::new("/etc/ssl/certs/cacert.pem").is_file() {
        command.arg("--cacert").arg("/etc/ssl/certs/cacert.pem");
    }
    command.arg(url);
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
}

fn temporary_headers_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let label = label.replace(' ', "-");
    PathBuf::from(format!(
        "/tmp/mister-magik-{label}-{}-{stamp}.headers",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_stderr_is_fully_drained_but_retained_output_is_bounded() {
        let bytes = std::io::Cursor::new(vec![b'x'; 256 * 1024]);
        let result = drain_curl_stderr(bytes).join().unwrap();
        assert_eq!(result.len(), 768);
        assert!(result.bytes().all(|byte| byte == b'x'));
    }
    use std::ffi::OsString;

    #[test]
    fn manifest_curl_is_https_only_and_bounded() {
        let mut command = Command::new("curl");
        add_https_fetch_args(
            &mut command,
            "https://assets.example/manifest.json",
            Path::new("/tmp/headers"),
            MAX_MANIFEST_BYTES,
        );
        let args = command.get_args().map(OsString::from).collect::<Vec<_>>();
        let text = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(text.contains("--proto =https"));
        assert!(text.contains("--proto-redir =https"));
        assert!(text.contains("--connect-timeout 10"));
        assert!(text.contains("--max-time 15"));
        assert!(text.contains("--max-filesize 262144"));
    }

    #[test]
    fn unsigned_manifest_fetch_skips_signature_and_verification() {
        let mut urls = Vec::new();
        let fetched = fetch_manifest_with(
            "https://assets.example/manifest.json",
            ManifestTrustMode::UnsignedHttps,
            |url, _, _| {
                urls.push(url.to_string());
                Ok(HttpsFetch {
                    bytes: b"unsigned manifest".to_vec(),
                    headers: "etag: test".to_string(),
                })
            },
            |_, _| panic!("unsigned mode must not verify a signature"),
        )
        .unwrap();

        assert_eq!(urls, ["https://assets.example/manifest.json"]);
        assert_eq!(fetched.bytes, b"unsigned manifest");
        assert_eq!(fetched.headers, "etag: test");
    }

    #[test]
    fn signed_manifest_fetch_requests_signature_and_verifies_raw_bytes() {
        let manifest = vec![0xff, 0x00, b'{'];
        let signature = b"signature envelope".to_vec();
        let mut urls = Vec::new();
        let fetched = fetch_manifest_with(
            "https://assets.example/manifest.json",
            ManifestTrustMode::SignedHttps,
            |url, _, label| {
                urls.push(url.to_string());
                Ok(HttpsFetch {
                    bytes: if label == "manifest" {
                        manifest.clone()
                    } else {
                        signature.clone()
                    },
                    headers: String::new(),
                })
            },
            |actual_manifest, actual_signature| {
                assert_eq!(actual_manifest, manifest);
                assert_eq!(actual_signature, signature);
                Ok("test-key".to_string())
            },
        )
        .unwrap();

        assert_eq!(
            urls,
            [
                "https://assets.example/manifest.json",
                "https://assets.example/manifest.json.sig"
            ]
        );
        assert_eq!(fetched.bytes, manifest);
    }

    #[test]
    fn signed_manifest_fetch_propagates_verification_failure() {
        let error = fetch_manifest_with(
            "https://assets.example/manifest.json",
            ManifestTrustMode::SignedHttps,
            |_, _, label| {
                Ok(HttpsFetch {
                    bytes: label.as_bytes().to_vec(),
                    headers: String::new(),
                })
            },
            |_, _| Err("invalid signature".to_string()),
        )
        .unwrap_err();

        assert_eq!(error, "invalid signature");
    }

    #[test]
    fn bounded_stream_chunk_accepts_exact_size_and_stops_before_overrun() {
        let mut output = Vec::new();
        let mut hash = Vec::new();
        let bytes =
            write_bounded_stream_chunk(&mut output, &mut hash, b"exact", 0, 5, "pack").unwrap();
        assert_eq!(bytes, 5);
        assert_eq!(output, b"exact");
        assert_eq!(hash, b"exact");

        let error = write_bounded_stream_chunk(&mut output, &mut hash, b"more", bytes, 5, "pack")
            .unwrap_err();
        assert!(error.contains("exceeds declared size"));
        assert_eq!(output, b"exact");
        assert_eq!(hash, b"exact");
    }
}
