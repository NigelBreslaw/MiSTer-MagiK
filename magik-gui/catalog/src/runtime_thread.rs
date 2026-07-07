//! Runtime thread scheduling policy for production background work.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeThreadRole {
    LauncherUi,
    CatalogWorker,
    CatalogForeground,
    LibraryWalker,
    LibraryWalkerForeground,
    PreviewSelected,
    PreviewPrefetch,
    MediaWorker,
    MediaDownload,
    MediaIndex,
    FramebufferStream,
    VideoDecode,
    VideoAudio,
}

impl RuntimeThreadRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::LauncherUi => "launcher-ui",
            Self::CatalogWorker => "catalog-worker",
            Self::CatalogForeground => "catalog-foreground",
            Self::LibraryWalker => "library-walker",
            Self::LibraryWalkerForeground => "library-walker-foreground",
            Self::PreviewSelected => "preview-selected",
            Self::PreviewPrefetch => "preview-prefetch",
            Self::MediaWorker => "media-worker",
            Self::MediaDownload => "media-download",
            Self::MediaIndex => "media-index",
            Self::FramebufferStream => "framebuffer-stream",
            Self::VideoDecode => "video-decode",
            Self::VideoAudio => "video-audio",
        }
    }

    pub fn default_policy(self) -> RuntimeThreadPolicy {
        match self {
            Self::LauncherUi => RuntimeThreadPolicy::new(-10, ThreadAffinity::Any),
            Self::CatalogWorker => RuntimeThreadPolicy::new(5, ThreadAffinity::Cpu0),
            Self::CatalogForeground | Self::LibraryWalkerForeground => {
                RuntimeThreadPolicy::new(0, ThreadAffinity::Any)
            }
            Self::LibraryWalker => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0),
            Self::PreviewSelected => RuntimeThreadPolicy::new(0, ThreadAffinity::Any),
            Self::PreviewPrefetch => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0),
            Self::MediaWorker | Self::MediaIndex => {
                RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0)
            }
            Self::FramebufferStream => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0),
            Self::MediaDownload => RuntimeThreadPolicy::new(0, ThreadAffinity::Any),
            Self::VideoDecode | Self::VideoAudio => {
                RuntimeThreadPolicy::new(5, ThreadAffinity::Any)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeThreadPolicy {
    pub nice: i32,
    pub affinity: ThreadAffinity,
}

impl RuntimeThreadPolicy {
    const fn new(nice: i32, affinity: ThreadAffinity) -> Self {
        Self { nice, affinity }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadAffinity {
    Any,
    Cpu0,
}

impl ThreadAffinity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Cpu0 => "cpu0",
        }
    }
}

pub fn apply_runtime_thread_policy(role: RuntimeThreadRole) {
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string();
    if policy_disabled() {
        let actual_nice = current_nice();
        let processor = current_processor();
        crate::catalog_logln!(
            "thread_policy_tsv\tthread={thread_name}\trole={}\tintended_nice=inherit\tactual_nice={}\taffinity=any\tprocessor={}\tnice_status=skipped\taffinity_status=skipped",
            role.label(),
            actual_nice.map_or_else(|| "unknown".to_string(), |nice| nice.to_string()),
            processor.map_or_else(|| "unknown".to_string(), |cpu| cpu.to_string())
        );
        return;
    }
    let policy = resolved_policy(role);
    let nice_status = apply_nice(policy.nice);
    let affinity_status = apply_affinity(policy.affinity);
    let actual_nice = current_nice();
    let processor = current_processor();
    crate::catalog_logln!(
        "thread_policy_tsv\tthread={thread_name}\trole={}\tintended_nice={}\tactual_nice={}\taffinity={}\tprocessor={}\tnice_status={nice_status}\taffinity_status={affinity_status}",
        role.label(),
        policy.nice,
        actual_nice.map_or_else(|| "unknown".to_string(), |nice| nice.to_string()),
        policy.affinity.label(),
        processor.map_or_else(|| "unknown".to_string(), |cpu| cpu.to_string())
    );
}

fn resolved_policy(role: RuntimeThreadRole) -> RuntimeThreadPolicy {
    let mut policy = role.default_policy();
    if affinity_disabled() {
        policy.affinity = ThreadAffinity::Any;
    }
    policy
}

fn policy_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| env_flag_is_off("MISTER_THREAD_POLICY"))
}

fn affinity_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| env_flag_is_off("MISTER_BACKGROUND_AFFINITY"))
}

fn env_flag_is_off(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("0") | Ok("off") | Ok("false") | Ok("no") | Ok("any")
    )
}

#[cfg(target_os = "linux")]
fn apply_nice(nice: i32) -> &'static str {
    // SAFETY: setpriority only adjusts the current thread/process scheduling
    // nice value. Failure is non-fatal; the worker continues at its inherited
    // priority and the status is emitted for benchmarks.
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) };
    if rc == 0 {
        "ok"
    } else {
        "failed"
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_nice(_nice: i32) -> &'static str {
    "unsupported"
}

#[cfg(target_os = "linux")]
fn apply_affinity(affinity: ThreadAffinity) -> &'static str {
    match affinity {
        ThreadAffinity::Any => "skipped",
        ThreadAffinity::Cpu0 => {
            // SAFETY: cpu_set_t is a plain C bitset. sched_setaffinity with pid
            // 0 targets the current thread on Linux; failure is non-fatal.
            unsafe {
                let mut set: libc::cpu_set_t = std::mem::zeroed();
                libc::CPU_ZERO(&mut set);
                libc::CPU_SET(0, &mut set);
                let rc = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
                if rc == 0 {
                    "ok"
                } else {
                    "failed"
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_affinity(_affinity: ThreadAffinity) -> &'static str {
    "unsupported"
}

#[cfg(target_os = "linux")]
fn current_nice() -> Option<i32> {
    // SAFETY: getpriority reads the current thread/process priority.
    Some(unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) })
}

#[cfg(not(target_os = "linux"))]
fn current_nice() -> Option<i32> {
    None
}

#[cfg(target_os = "linux")]
fn current_processor() -> Option<i32> {
    // SAFETY: sched_getcpu reads the current CPU number.
    let cpu = unsafe { libc::sched_getcpu() };
    (cpu >= 0).then_some(cpu)
}

#[cfg(not(target_os = "linux"))]
fn current_processor() -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_preview_runs_at_interactive_priority() {
        assert_eq!(
            RuntimeThreadRole::PreviewSelected.default_policy(),
            RuntimeThreadPolicy::new(0, ThreadAffinity::Any)
        );
        assert_eq!(
            RuntimeThreadRole::PreviewPrefetch.default_policy(),
            RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0)
        );
    }

    #[test]
    fn launcher_ui_runs_above_default_interactive_priority() {
        assert_eq!(
            RuntimeThreadRole::LauncherUi.default_policy(),
            RuntimeThreadPolicy::new(-10, ThreadAffinity::Any)
        );
    }

    #[test]
    fn heavy_background_roles_default_to_cpu0_affinity() {
        for role in [
            RuntimeThreadRole::CatalogWorker,
            RuntimeThreadRole::LibraryWalker,
            RuntimeThreadRole::MediaWorker,
            RuntimeThreadRole::MediaIndex,
            RuntimeThreadRole::FramebufferStream,
        ] {
            assert_eq!(role.default_policy().affinity, ThreadAffinity::Cpu0);
            assert!(role.default_policy().nice >= 5);
        }
    }

    #[test]
    fn visible_media_download_runs_at_interactive_priority() {
        assert_eq!(
            RuntimeThreadRole::MediaDownload.default_policy(),
            RuntimeThreadPolicy::new(0, ThreadAffinity::Any)
        );
    }

    #[test]
    fn first_catalog_build_roles_run_foreground() {
        for role in [
            RuntimeThreadRole::CatalogForeground,
            RuntimeThreadRole::LibraryWalkerForeground,
        ] {
            assert_eq!(
                role.default_policy(),
                RuntimeThreadPolicy::new(0, ThreadAffinity::Any)
            );
        }
    }

    #[test]
    fn env_off_values_are_recognized() {
        for value in ["0", "off", "false", "no", "any"] {
            std::env::set_var("MISTER_THREAD_POLICY", value);
            assert!(env_flag_is_off("MISTER_THREAD_POLICY"));
        }
        std::env::set_var("MISTER_THREAD_POLICY", "on");
        assert!(!env_flag_is_off("MISTER_THREAD_POLICY"));
        std::env::remove_var("MISTER_THREAD_POLICY");
    }
}
