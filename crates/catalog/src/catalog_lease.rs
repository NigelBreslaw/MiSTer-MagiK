// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exclusive ownership for catalog mutation.
//!
//! The lock is advisory and held by the process that performs generation
//! selection and publication.  The diagnostic lock-file contents are never
//! used to infer ownership; the kernel lock is the authority and is released
//! automatically when the owning process exits.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_CATALOG_BUILDER_LOCK_PATH: &str = "/tmp/mister-magik/catalog-builder.lock";
const CATALOG_BUILDER_LOCK_ENV: &str = "MISTER_CATALOG_BUILDER_LOCK";
static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// A process-local identity used to keep temporary publication files unique.
///
/// The value is deliberately opaque: it is only ever embedded in filenames and
/// diagnostics, never parsed as a generation or used for authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogRunId(String);

impl CatalogRunId {
    pub fn new() -> Self {
        let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        Self(format!("{}-{now:x}-{sequence:x}", std::process::id()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CatalogRunId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub enum CatalogLeaseError {
    Busy { path: PathBuf },
    Io { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for CatalogLeaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { path } => write!(formatter, "catalog builder is busy: {}", path.display()),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "catalog builder lock {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for CatalogLeaseError {}

pub struct CatalogMutationLease {
    #[cfg(test)]
    path: PathBuf,
    _file: File,
}

impl CatalogMutationLease {
    pub fn acquire_default() -> Result<Self, CatalogLeaseError> {
        let path = std::env::var_os(CATALOG_BUILDER_LOCK_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                #[cfg(test)]
                {
                    let thread =
                        format!("{:?}", std::thread::current().id()).replace(['(', ')'], "");
                    return std::env::temp_dir()
                        .join(format!("mister-magik-catalog-builder-test-{thread}.lock"));
                }
                #[cfg(not(test))]
                {
                    PathBuf::from(DEFAULT_CATALOG_BUILDER_LOCK_PATH)
                }
            });
        Self::acquire(path)
    }

    pub(crate) fn acquire(path: impl Into<PathBuf>) -> Result<Self, CatalogLeaseError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| CatalogLeaseError::Io {
                path: path.clone(),
                source,
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .map_err(|source| CatalogLeaseError::Io {
                path: path.clone(),
                source,
            })?;
        let descriptor_flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        if descriptor_flags < 0 {
            return Err(CatalogLeaseError::Io {
                path,
                source: io::Error::last_os_error(),
            });
        }
        let result = unsafe {
            libc::fcntl(
                file.as_raw_fd(),
                libc::F_SETFD,
                descriptor_flags | libc::FD_CLOEXEC,
            )
        };
        if result != 0 {
            return Err(CatalogLeaseError::Io {
                path,
                source: io::Error::last_os_error(),
            });
        }
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let source = io::Error::last_os_error();
            if source.raw_os_error() == Some(libc::EWOULDBLOCK)
                || source.raw_os_error() == Some(libc::EAGAIN)
            {
                return Err(CatalogLeaseError::Busy { path });
            }
            return Err(CatalogLeaseError::Io { path, source });
        }
        Ok(Self {
            #[cfg(test)]
            path,
            _file: file,
        })
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn file(&self) -> &File {
        &self._file
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn second_lease_is_busy_until_first_is_dropped() {
        let path = unique_path("catalog-lease");
        let first = CatalogMutationLease::acquire(&path).expect("first lease");
        assert!(matches!(
            CatalogMutationLease::acquire(&path),
            Err(CatalogLeaseError::Busy { .. })
        ));
        drop(first);
        let second = CatalogMutationLease::acquire(&path).expect("released lease");
        assert_eq!(second.path(), path);
        let _ = fs::remove_file(path);
    }

    #[test]
    #[cfg(unix)]
    fn lease_descriptor_is_close_on_exec() {
        let path = unique_path("catalog-lease-cloexec");
        let lease = CatalogMutationLease::acquire(&path).expect("lease");
        let flags = unsafe { libc::fcntl(lease.file().as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
        let _ = fs::remove_file(path);
    }
}
