use ssh2::{ExtendedData, Session};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::Result;

pub(crate) fn host() -> String {
    env::var("MISTER_IP").unwrap_or_else(|_| "192.168.1.117".to_string())
}

fn user() -> String {
    env::var("MISTER_USER").unwrap_or_else(|_| "root".to_string())
}

fn pass() -> String {
    env::var("MISTER_PASS").unwrap_or_else(|_| "1".to_string())
}

pub(crate) fn connect(timeout_secs: u64) -> Result<Session> {
    let addr = format!("{}:22", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer host")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(timeout_secs))?;
    tcp.set_read_timeout(Some(Duration::from_secs(timeout_secs)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(timeout_secs)))?;
    let mut sess = Session::new()?;
    sess.set_tcp_stream(tcp);
    sess.handshake()?;
    sess.userauth_password(&user(), &pass())?;
    if !sess.authenticated() {
        return Err("SSH password authentication failed".into());
    }
    Ok(sess)
}

pub(crate) struct ExecOutput {
    pub(crate) rc: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn exec(sess: &Session, command: &str, merge_stderr: bool) -> Result<ExecOutput> {
    let mut channel = sess.channel_session()?;
    if merge_stderr {
        channel.handle_extended_data(ExtendedData::Merge)?;
    }
    channel.exec(command)?;
    let mut stdout = String::new();
    channel.read_to_string(&mut stdout)?;
    let mut stderr = String::new();
    if !merge_stderr {
        channel.stderr().read_to_string(&mut stderr)?;
    }
    channel.wait_close()?;
    Ok(ExecOutput {
        rc: channel.exit_status()?,
        stdout,
        stderr,
    })
}

pub(crate) fn stream_command(sess: &Session, command: &str) -> Result<()> {
    let mut channel = sess.channel_session()?;
    channel.handle_extended_data(ExtendedData::Merge)?;
    channel.exec(command)?;
    let mut buf = [0u8; 8192];
    loop {
        match channel.read(&mut buf) {
            Ok(0) => {
                if channel.eof() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(n) => {
                io::stdout().write_all(&buf[..n])?;
                io::stdout().flush()?;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
    channel.wait_close()?;
    std::process::exit(channel.exit_status()?);
}

pub(crate) fn put(sess: &Session, local: &Path, remote: &str) -> Result<()> {
    let sftp = sess.sftp()?;
    ensure_remote_parent_dir(&sftp, Path::new(remote))?;
    let mut src = File::open(local)?;
    let mut dst = sftp.create(Path::new(remote))?;
    io::copy(&mut src, &mut dst)?;
    Ok(())
}

pub(crate) fn put_bytes(sess: &Session, remote: &str, bytes: &[u8]) -> Result<()> {
    let sftp = sess.sftp()?;
    put_bytes_with_sftp(&sftp, remote, bytes)
}

fn put_bytes_with_sftp(sftp: &ssh2::Sftp, remote: &str, bytes: &[u8]) -> Result<()> {
    ensure_remote_parent_dir(sftp, Path::new(remote))?;
    let mut dst = sftp.create(Path::new(remote))?;
    dst.write_all(bytes)?;
    Ok(())
}

pub(crate) fn put_dir(sess: &Session, local_dir: &Path, remote_dir: &str) -> Result<usize> {
    let sftp = sess.sftp()?;
    if !local_dir.is_dir() {
        return Err(format!("{} is not a directory", local_dir.display()).into());
    }
    ensure_remote_dir(&sftp, Path::new(remote_dir))?;
    let mut count = 0;
    put_dir_recursive(
        &sftp,
        local_dir,
        local_dir,
        Path::new(remote_dir),
        &mut count,
    )?;
    Ok(count)
}

fn put_dir_recursive(
    sftp: &ssh2::Sftp,
    root: &Path,
    dir: &Path,
    remote_root: &Path,
    count: &mut usize,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            put_dir_recursive(sftp, root, &path, remote_root, count)?;
        } else if metadata.is_file() {
            let rel = path.strip_prefix(root)?;
            let remote = remote_root.join(rel);
            ensure_remote_parent_dir(sftp, &remote)?;
            let mut src = File::open(&path)?;
            let mut dst = sftp.create(&remote)?;
            io::copy(&mut src, &mut dst)?;
            *count += 1;
        }
    }
    Ok(())
}

fn ensure_remote_parent_dir(sftp: &ssh2::Sftp, remote: &Path) -> Result<()> {
    if let Some(parent) = remote.parent() {
        ensure_remote_dir(sftp, parent)?;
    }
    Ok(())
}

fn ensure_remote_dir(sftp: &ssh2::Sftp, remote: &Path) -> Result<()> {
    if remote.as_os_str().is_empty() || remote == Path::new("/") {
        return Ok(());
    }
    if sftp.stat(remote).is_ok() {
        return Ok(());
    }
    if let Some(parent) = remote.parent() {
        ensure_remote_dir(sftp, parent)?;
    }
    match sftp.mkdir(remote, 0o755) {
        Ok(()) => Ok(()),
        Err(_) if sftp.stat(remote).is_ok() => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn get(sess: &Session, remote: &str, local: &Path) -> Result<()> {
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent)?;
    }
    let sftp = sess.sftp()?;
    let mut src = sftp.open(Path::new(remote))?;
    let mut dst = File::create(local)?;
    io::copy(&mut src, &mut dst)?;
    Ok(())
}

pub(crate) struct TimedSession {
    pub(crate) sess: Session,
    pub(crate) resolve_ms: u128,
    pub(crate) tcp_ms: u128,
    pub(crate) handshake_ms: u128,
    pub(crate) auth_ms: u128,
}

pub(crate) fn connect_timed(timeout_secs: u64) -> Result<TimedSession> {
    let resolve_t = Instant::now();
    let addr = format!("{}:22", host())
        .to_socket_addrs()?
        .next()
        .ok_or("could not resolve MiSTer host")?;
    let resolve_ms = resolve_t.elapsed().as_millis();

    let tcp_t = Instant::now();
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(timeout_secs))?;
    let tcp_ms = tcp_t.elapsed().as_millis();
    tcp.set_read_timeout(Some(Duration::from_secs(timeout_secs)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(timeout_secs)))?;

    let mut sess = Session::new()?;
    sess.set_tcp_stream(tcp);
    let handshake_t = Instant::now();
    sess.handshake()?;
    let handshake_ms = handshake_t.elapsed().as_millis();
    let auth_t = Instant::now();
    sess.userauth_password(&user(), &pass())?;
    let auth_ms = auth_t.elapsed().as_millis();
    if !sess.authenticated() {
        return Err("SSH password authentication failed".into());
    }
    Ok(TimedSession {
        sess,
        resolve_ms,
        tcp_ms,
        handshake_ms,
        auth_ms,
    })
}

pub(crate) fn sftp_write_profile(sess: &Session, remote: &str, bytes: &[u8]) -> Result<u128> {
    let sftp = sess.sftp()?;
    let t = Instant::now();
    let mut dst = sftp.create(Path::new(remote))?;
    dst.write_all(bytes)?;
    Ok(t.elapsed().as_millis())
}

pub(crate) fn tcp_probe_label(timeout: Duration) -> String {
    tcp_probe_label_port(22, timeout)
}

pub(crate) fn tcp_probe_label_port(port: u16, timeout: Duration) -> String {
    let addr = match format!("{}:{port}", host()).to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => addr,
            None => return "resolve_none".to_string(),
        },
        Err(err) => return format!("resolve_{}", err.kind() as u8),
    };

    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(_) => "ok".to_string(),
        Err(err) => match err.raw_os_error() {
            Some(64) => "hostdown".to_string(),
            Some(65) => "noroute".to_string(),
            Some(60) => "timeout".to_string(),
            Some(61) => "refused".to_string(),
            Some(code) => format!("os{code}"),
            None if err.kind() == io::ErrorKind::TimedOut => "timeout".to_string(),
            None if err.kind() == io::ErrorKind::ConnectionRefused => "refused".to_string(),
            None => format!("{:?}", err.kind()).to_lowercase(),
        },
    }
}

pub(crate) fn host_wait_diagnostics() -> String {
    let host = host();
    let tcp = tcp_probe_label(Duration::from_millis(500));
    let arp = command_summary("arp", &["-an"], Some(&host));
    let ping = if cfg!(target_os = "macos") {
        command_summary("ping", &["-c", "1", "-W", "1000", &host], None)
    } else {
        command_summary("ping", &["-c", "1", "-W", "1", &host], None)
    };
    format!("tcp={tcp}; arp={arp}; ping={ping}")
}

fn command_summary(program: &str, args: &[&str], contains: Option<&str>) -> String {
    match Command::new(program).args(args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut lines = stdout
                .lines()
                .chain(stderr.lines())
                .filter(|line| contains.map(|needle| line.contains(needle)).unwrap_or(true))
                .map(str::trim)
                .filter(|line| !line.is_empty());
            let text = lines.next().unwrap_or("no matching output");
            format!(
                "rc={} {}",
                output.status.code().unwrap_or(-1),
                text.replace('\t', " ")
            )
        }
        Err(err) => format!("error={}", err),
    }
}

pub(crate) fn port_open(timeout: Duration) -> bool {
    let Ok(mut addrs) = format!("{}:22", host()).to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}
