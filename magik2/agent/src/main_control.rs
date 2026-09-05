//! The installed Main FIFO contract, with one bounded lock/write/reply budget.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

pub fn handoff(command: &str) -> Result<(), String> {
    exchange(
        command,
        Path::new("/dev/MiSTer_cmd"),
        Path::new("/dev/MiSTer_cmd_reply"),
        Path::new("/tmp/mister-magik/command-operation.lock"),
        Duration::from_secs(5),
    )
}

fn exchange(
    command: &str,
    fifo: &Path,
    replies: &Path,
    lock: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock)
        .map_err(|e| e.to_string())?;
    loop {
        // SAFETY: the file descriptor is live; flock does not retain pointers.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        if Instant::now() >= deadline {
            return Err("Main command lane is busy".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut reply = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(replies)
        .map_err(|e| e.to_string())?;
    let mut chunk = [0; 512];
    while reply.read(&mut chunk).is_ok_and(|n| n > 0) {
        if Instant::now() >= deadline {
            return Err("Main reply drain exceeded deadline".into());
        }
    }
    let mut writer = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(fifo)
        .map_err(|e| format!("Main command unavailable: {e}"))?;
    writer
        .write_all(command.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        match reply.read(&mut chunk) {
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if let Some(end) = bytes.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&bytes[..end]);
            return if line == "ok" || line.starts_with("ok ") {
                Ok(())
            } else {
                Err(format!("Main rejected command: {line}"))
            };
        }
        if bytes.len() > 512 {
            return Err("Main reply exceeds bound".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("Main did not acknowledge command before deadline".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_fifo_reader_is_bounded() {
        let root = std::env::temp_dir().join(format!("magik2-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let fifo = root.join("command");
        let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: name is a live null-terminated pathname.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        std::fs::File::create(root.join("reply")).unwrap();
        let started = Instant::now();
        assert!(
            exchange(
                "mister_magik_suspend\n",
                &fifo,
                &root.join("reply"),
                &root.join("lock"),
                Duration::from_millis(100)
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        std::fs::remove_dir_all(root).unwrap();
    }
}
