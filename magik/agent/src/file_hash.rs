//! SHA-256 of installed files, reused while a file's identity is unchanged.
//!
//! Executables are tens of MB and every host connection asks for status and
//! platform state. Publication renames a new file into place, so a replaced
//! artifact has a different inode even when its size and FAT mtime match.
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const MAX_CACHED_HASHES: usize = 32;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    len: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileIdentity {
    fn of(path: &Path) -> Result<Self, String> {
        let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            len: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[cfg(test)]
thread_local! {
    /// Full-file digests computed on this thread; proves cache hits in tests.
    pub(crate) static DIGESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn digest(path: &Path) -> Result<String, String> {
    #[cfg(test)]
    DIGESTS.set(DIGESTS.get() + 1);
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(crate) fn sha256(path: &Path) -> Result<String, String> {
    static CACHE: OnceLock<Mutex<HashMap<FileIdentity, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let before = FileIdentity::of(path)?;
    if let Some(hash) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&before) {
        return Ok(hash.clone());
    }
    let hash = digest(path)?;
    // Only reuse a hash whose file did not change while it was being read.
    if FileIdentity::of(path).as_ref() == Ok(&before) {
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= MAX_CACHED_HASHES {
            cache.clear();
        }
        cache.insert(before, hash.clone());
    }
    Ok(hash)
}
