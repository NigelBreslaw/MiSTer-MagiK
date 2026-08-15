// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

// Main_MiSTer now suppresses its OSD/menu/framebuffer paths while the MagiK
// launcher is active, so periodic route reassertion is a diagnostic fallback
// rather than normal steady-state work. Set MISTER_FB_ROUTE_REASSERT_FRAMES to a
// positive frame interval to re-enable the watchdog during attended debugging.
pub const DEFAULT_REASSERT_FRAMES: u64 = 0;
pub const DEFAULT_DISPLAY_OWNER_LOCK_PATH: &str = "/tmp/mister-magik/display-owner.lock";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferRouteConfig {
    reassert_interval_frames: u64,
}

impl FramebufferRouteConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self {
            reassert_interval_frames: get("MISTER_FB_ROUTE_REASSERT_FRAMES")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_REASSERT_FRAMES),
        }
    }

    pub fn reassert_interval_frames(self) -> u64 {
        self.reassert_interval_frames
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferRouteAction {
    pub reassert_route: bool,
    pub force_full_present: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FramebufferRouteGuard {
    interval_frames: u64,
    next_frame: u64,
}

impl FramebufferRouteGuard {
    pub fn new(interval_frames: u64) -> Self {
        Self {
            interval_frames,
            next_frame: 0,
        }
    }

    pub fn from_env() -> Self {
        Self::new(reassert_interval_frames_from_env())
    }

    pub const fn disabled() -> Self {
        Self {
            interval_frames: 0,
            next_frame: u64::MAX,
        }
    }

    pub fn tick(&mut self, frame: u64) -> FramebufferRouteAction {
        if self.interval_frames == 0 || frame < self.next_frame {
            return FramebufferRouteAction {
                reassert_route: false,
                force_full_present: false,
            };
        }

        self.next_frame = frame.saturating_add(self.interval_frames.max(1));
        FramebufferRouteAction {
            reassert_route: true,
            force_full_present: true,
        }
    }
}

pub fn reassert_interval_frames_from_env() -> u64 {
    let values = std::env::vars().collect::<std::collections::HashMap<_, _>>();
    FramebufferRouteConfig::capture_with(|name| values.get(name).map(String::as_str))
        .reassert_interval_frames()
}

pub fn reassert_interval_duration(frames: u64, refresh_hz: u64) -> Option<Duration> {
    if frames == 0 || refresh_hz == 0 {
        return None;
    }
    Some(Duration::from_millis(
        frames.saturating_mul(1000) / refresh_hz,
    ))
}

pub fn should_present_full_frame(launching: bool, route_action: FramebufferRouteAction) -> bool {
    launching || route_action.force_full_present
}

#[derive(Debug)]
pub struct DisplayOwnerLock {
    file: File,
    path: PathBuf,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ActiveDisplayOwner {
    pub path: PathBuf,
    pub pid: Option<u32>,
    pub cmdline: Option<String>,
}

#[derive(Debug)]
pub enum DisplayOwnerLockError {
    Active(ActiveDisplayOwner),
    Io(std::io::Error),
}

impl std::fmt::Display for DisplayOwnerLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active(owner) => {
                write!(f, "{} is already locked", owner.path.display())?;
                if let Some(pid) = owner.pid {
                    write!(f, " by pid {pid}")?;
                }
                if let Some(cmdline) = owner.cmdline.as_deref().filter(|cmd| !cmd.is_empty()) {
                    write!(f, " ({cmdline})")?;
                }
                Ok(())
            }
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DisplayOwnerLockError {}

impl DisplayOwnerLock {
    pub fn acquire_default() -> Result<Self, DisplayOwnerLockError> {
        let path = std::env::var("MISTER_DISPLAY_OWNER_LOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_DISPLAY_OWNER_LOCK_PATH));
        Self::acquire(&path)
    }

    pub fn acquire(path: &Path) -> Result<Self, DisplayOwnerLockError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(DisplayOwnerLockError::Io)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(DisplayOwnerLockError::Io)?;

        if !try_lock_exclusive_nonblocking(&file).map_err(DisplayOwnerLockError::Io)? {
            return Err(DisplayOwnerLockError::Active(read_active_owner(
                path, &mut file,
            )));
        }

        file.set_len(0).map_err(DisplayOwnerLockError::Io)?;
        file.seek(SeekFrom::Start(0))
            .map_err(DisplayOwnerLockError::Io)?;
        writeln!(file, "{}", std::process::id()).map_err(DisplayOwnerLockError::Io)?;
        file.sync_data().map_err(DisplayOwnerLockError::Io)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DisplayOwnerLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self.file);
    }
}

fn try_lock_exclusive_nonblocking(file: &File) -> std::io::Result<bool> {
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    ) {
        Ok(false)
    } else {
        Err(error)
    }
}

fn unlock_file(file: &File) -> std::io::Result<()> {
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn read_active_owner(path: &Path, file: &mut File) -> ActiveDisplayOwner {
    let pid = read_lock_pid_from_file(file);
    ActiveDisplayOwner {
        path: path.to_path_buf(),
        pid,
        cmdline: pid.and_then(read_process_cmdline),
    }
}

fn read_lock_pid_from_file(file: &mut File) -> Option<u32> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut text).ok()?;
    text.trim().parse::<u32>().ok()
}

fn read_process_cmdline(pid: u32) -> Option<String> {
    let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let parts = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_lock_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{nanos}.lock",
            std::process::id()
        ))
    }

    #[test]
    fn guard_reasserts_on_first_frame() {
        let mut guard = FramebufferRouteGuard::new(60);

        assert_eq!(
            guard.tick(0),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
    }

    #[test]
    fn guard_waits_until_interval_elapses() {
        let mut guard = FramebufferRouteGuard::new(3);

        assert!(guard.tick(0).reassert_route);
        assert!(!guard.tick(1).reassert_route);
        assert!(!guard.tick(2).reassert_route);
        assert_eq!(
            guard.tick(3),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
    }

    #[test]
    fn periodic_route_reassertions_force_full_presents() {
        let mut guard = FramebufferRouteGuard::new(2);

        assert_eq!(
            guard.tick(0),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
        assert_eq!(
            guard.tick(2),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
        assert_eq!(
            guard.tick(4),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
    }

    #[test]
    fn disabled_guard_never_reasserts() {
        let mut guard = FramebufferRouteGuard::disabled();

        for frame in [0, 1, 60, u64::MAX - 1] {
            assert_eq!(
                guard.tick(frame),
                FramebufferRouteAction {
                    reassert_route: false,
                    force_full_present: false
                }
            );
        }
    }

    #[test]
    fn default_interval_disables_periodic_reassertion() {
        assert_eq!(DEFAULT_REASSERT_FRAMES, 0);
        assert_eq!(
            reassert_interval_duration(DEFAULT_REASSERT_FRAMES, 60),
            None
        );
    }

    #[test]
    fn interval_duration_handles_disabled_values() {
        assert_eq!(reassert_interval_duration(0, 60), None);
        assert_eq!(reassert_interval_duration(60, 0), None);
        assert_eq!(
            reassert_interval_duration(60, 60),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn full_frame_present_follows_launch_or_explicit_action() {
        assert!(should_present_full_frame(
            false,
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        ));
        assert!(should_present_full_frame(
            true,
            FramebufferRouteAction {
                reassert_route: false,
                force_full_present: false
            }
        ));
        assert!(!should_present_full_frame(
            false,
            FramebufferRouteAction {
                reassert_route: false,
                force_full_present: false
            }
        ));
    }

    #[test]
    fn display_owner_lock_refuses_second_owner() {
        let path = unique_lock_path("display-owner-active");
        let first = DisplayOwnerLock::acquire(&path).expect("first lock");

        let error = DisplayOwnerLock::acquire(&path).expect_err("second lock refused");

        match error {
            DisplayOwnerLockError::Active(owner) => {
                assert_eq!(owner.path, path);
                assert_eq!(owner.pid, Some(std::process::id()));
            }
            other => panic!("unexpected error: {other}"),
        }
        drop(first);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn display_owner_lock_releases_on_drop() {
        let path = unique_lock_path("display-owner-release");
        let first = DisplayOwnerLock::acquire(&path).expect("first lock");
        drop(first);

        let second = DisplayOwnerLock::acquire(&path).expect("second lock after drop");

        drop(second);
        let _ = std::fs::remove_file(path);
    }
}
