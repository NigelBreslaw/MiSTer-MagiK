// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded reads of regular files, including a separate tail policy for logs.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug)]
pub struct SizeLimitExceeded {
    pub observed: u64,
    pub limit: u64,
}
impl std::fmt::Display for SizeLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "file size {} exceeds limit {}",
            self.observed, self.limit
        )
    }
}
impl std::error::Error for SizeLimitExceeded {}

pub fn size_limit(error: &io::Error) -> Option<&SizeLimitExceeded> {
    error.get_ref()?.downcast_ref()
}

fn buffer(capacity: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(usize::try_from(capacity).map_err(io::Error::other)?)
        .map_err(io::Error::other)?;
    Ok(bytes)
}

fn regular_file(path: &Path) -> io::Result<(File, fs::Metadata)> {
    // Following a regular-file symlink remains supported. Inspect its target
    // before opening to avoid opening known device nodes. Nonblocking open
    // also closes the stat/open FIFO race; descriptor validation remains required.
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok((file, metadata))
}

/// Reject oversized inputs before allocating; the read limit also bounds a
/// file that grows after stat. Zero stat length (e.g. procfs) is supported.
pub fn read(path: impl AsRef<Path>, limit: u64) -> io::Result<Vec<u8>> {
    let (file, metadata) = regular_file(path.as_ref())?;
    let size = metadata.len();
    let oversized = |observed| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            SizeLimitExceeded { observed, limit },
        )
    };
    if size > limit {
        return Err(oversized(size));
    }
    let mut bytes = buffer(size.saturating_add(1))?;
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(oversized(bytes.len() as u64));
    }
    Ok(bytes)
}

/// Return at most `limit` bytes from the end of a regular log file.
/// Unlike `read`, a larger file is intentionally truncated rather than rejected.
pub fn read_tail(path: impl AsRef<Path>, limit: u64) -> io::Result<Vec<u8>> {
    let (mut file, metadata) = regular_file(path.as_ref())?;
    let start = metadata.len().saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = buffer(metadata.len().min(limit))?;
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_file_and_tail_keep_their_different_limit_contracts() {
        let root = crate::test_support::unique_temp_dir("bounded");
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let path = root.join("input");
        fs::write(&path, b"abcdef").unwrap();
        assert_eq!(read(&path, 6).unwrap(), b"abcdef");
        let error = read(&path, 5).unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<SizeLimitExceeded>()
                .unwrap()
                .observed,
            6
        );
        assert_eq!(read_tail(&path, 3).unwrap(), b"def");
        assert_eq!(read_tail(&path, 0).unwrap(), b"");
        assert!(read(&root, 10).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.join("link");
            symlink(&path, &link).unwrap();
            assert_eq!(read(&link, 6).unwrap(), b"abcdef");
            let fifo = root.join("fifo");
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            // SAFETY: NUL-terminated path; this fixture has no writer, so a
            // blocking open would hang. Both direct and symlink reads reject it.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let fifo_link = root.join("fifo-link");
            symlink(&fifo, &fifo_link).unwrap();
            for p in [&fifo, &fifo_link] {
                let p = p.to_owned();
                let (send, receive) = std::sync::mpsc::channel();
                let reader = std::thread::spawn(move || {
                    send.send((
                        read(&p, 8).unwrap_err().kind(),
                        read_tail(&p, 8).unwrap_err().kind(),
                    ))
                    .unwrap();
                });
                let (whole, tail) = receive
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("FIFO read must not block");
                assert_eq!(whole, io::ErrorKind::InvalidInput);
                assert_eq!(tail, io::ErrorKind::InvalidInput);
                reader.join().unwrap();
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}
