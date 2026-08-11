// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Runtime thread scheduling policy for production background work.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeThreadRole {
    LauncherUi,
    InputReader,
    InputDiscovery,
    CatalogWorker,
    SystemEntryPrepare,
    CatalogForeground,
    SearchIndex,
    LibraryWalker,
    LibraryWalkerForeground,
    PreviewSelected,
    PreviewPrefetch,
    MediaWorker,
    MediaDownload,
    MediaIndex,
    FramebufferStream,
    RuntimeStatus,
    VideoDecode,
    VideoAudio,
    ScreensaverRenderer,
    ParticlePreparer,
    StartupIntroSnapshot,
    ScreensaverLoader,
    ScreensaverScaler,
}

impl RuntimeThreadRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::LauncherUi => "launcher-ui",
            Self::InputReader => "input-reader",
            Self::InputDiscovery => "input-discovery",
            Self::CatalogWorker => "catalog-worker",
            Self::SystemEntryPrepare => "system-entry-prepare",
            Self::CatalogForeground => "catalog-foreground",
            Self::SearchIndex => "search-index",
            Self::LibraryWalker => "library-walker",
            Self::LibraryWalkerForeground => "library-walker-foreground",
            Self::PreviewSelected => "preview-selected",
            Self::PreviewPrefetch => "preview-prefetch",
            Self::MediaWorker => "media-worker",
            Self::MediaDownload => "media-download",
            Self::MediaIndex => "media-index",
            Self::FramebufferStream => "framebuffer-stream",
            Self::RuntimeStatus => "runtime-status",
            Self::VideoDecode => "video-decode",
            Self::VideoAudio => "video-audio",
            Self::ScreensaverRenderer => "screensaver-renderer",
            Self::ParticlePreparer => "particle-preparer",
            Self::StartupIntroSnapshot => "startup-intro-snapshot",
            Self::ScreensaverLoader => "screensaver-loader",
            Self::ScreensaverScaler => "screensaver-scaler",
        }
    }

    pub fn default_policy(self) -> RuntimeThreadPolicy {
        match self {
            Self::LauncherUi => RuntimeThreadPolicy::new(-10, ThreadAffinity::Cpu1),
            // The input proxy IRQ/wake path can leave a runnable CPU0 reader
            // behind tens of milliseconds of kernel/catalog work.  Keep the
            // reader with the latency-critical launcher work on CPU1; the
            // forced-catalog device laboratory bounds this wake at <1 ms.
            Self::InputReader => RuntimeThreadPolicy::new(-15, ThreadAffinity::Cpu1),
            // Raw hotplug discovery walks /dev and sysfs. It is never part of
            // navigation capture, so keep it away from the launcher/input CPU.
            Self::InputDiscovery => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0),
            Self::CatalogWorker => RuntimeThreadPolicy::new(5, ThreadAffinity::Cpu0),
            Self::SystemEntryPrepare => RuntimeThreadPolicy::new(0, ThreadAffinity::Cpu0),
            // Initial index construction is part of making a newly published
            // catalog fully usable. Give it both A9 cores until the P4
            // coordinator can yield it to an actual foreground request.
            Self::SearchIndex => RuntimeThreadPolicy::new(0, ThreadAffinity::AllOnline),
            Self::CatalogForeground | Self::LibraryWalkerForeground => {
                RuntimeThreadPolicy::new(0, ThreadAffinity::AllOnline)
            }
            Self::LibraryWalker => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0),
            Self::PreviewSelected => RuntimeThreadPolicy::new(0, ThreadAffinity::Inherit),
            Self::PreviewPrefetch => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0),
            Self::MediaWorker | Self::MediaIndex => {
                RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0)
            }
            Self::FramebufferStream => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0),
            // Status serialization and filesystem publication are periodic,
            // bursty work. Keep them off the CPU0 screensaver render worker;
            // CPU1 has ample launcher slack while the particle experiment is
            // active.
            Self::RuntimeStatus => RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu1),
            // Download starts only after it has joined the cooperative work
            // coordinator.  It may then use both online Cortex-A9 cores while
            // yielding at bounded stream-copy units for selected previews.
            Self::MediaDownload => RuntimeThreadPolicy::new(0, ThreadAffinity::AllOnline),
            Self::VideoDecode | Self::VideoAudio => {
                RuntimeThreadPolicy::new(5, ThreadAffinity::Inherit)
            }
            Self::ScreensaverRenderer => RuntimeThreadPolicy::new(-5, ThreadAffinity::Cpu0),
            Self::ParticlePreparer => RuntimeThreadPolicy::new(-5, ThreadAffinity::Cpu1),
            Self::StartupIntroSnapshot | Self::ScreensaverLoader | Self::ScreensaverScaler => {
                RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu0)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeThreadPolicy {
    pub nice: i32,
    pub affinity: ThreadAffinity,
    pub scheduler: ThreadScheduler,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeThreadPolicyReport {
    pub role: &'static str,
    pub intended_nice: Option<i32>,
    pub actual_nice: Option<i32>,
    pub intended_affinity: &'static str,
    pub allowed_cpus: String,
    pub processor: Option<i32>,
    pub scheduler_policy: Option<i32>,
    pub scheduler_priority: Option<i32>,
    pub thread_id: Option<i32>,
    pub nice_status: &'static str,
    pub affinity_status: &'static str,
    pub intended_scheduler: &'static str,
    pub scheduler_status: &'static str,
}

impl RuntimeThreadPolicy {
    pub const fn new(nice: i32, affinity: ThreadAffinity) -> Self {
        Self {
            nice,
            affinity,
            scheduler: ThreadScheduler::Other,
        }
    }

    pub const fn with_scheduler(mut self, scheduler: ThreadScheduler) -> Self {
        self.scheduler = scheduler;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadScheduler {
    Other,
    RoundRobin { priority: i32 },
}

impl ThreadScheduler {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::RoundRobin { .. } => "round-robin",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadAffinity {
    Inherit,
    AllOnline,
    Cpu0,
    Cpu1,
}

impl ThreadAffinity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::AllOnline => "all-online",
            Self::Cpu0 => "cpu0",
            Self::Cpu1 => "cpu1",
        }
    }
}

pub fn apply_runtime_thread_policy(role: RuntimeThreadRole) -> RuntimeThreadPolicyReport {
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string();
    if policy_disabled() {
        let actual_nice = current_nice();
        let processor = current_processor();
        let report = RuntimeThreadPolicyReport {
            role: role.label(),
            intended_nice: None,
            actual_nice,
            intended_affinity: "inherit",
            allowed_cpus: current_allowed_cpu_list(),
            processor,
            scheduler_policy: current_scheduler_policy(),
            scheduler_priority: current_scheduler_priority(),
            thread_id: current_thread_id(),
            nice_status: "skipped",
            affinity_status: "skipped",
            intended_scheduler: "inherit",
            scheduler_status: "skipped",
        };
        crate::catalog_logln!(
            "thread_policy_tsv\tthread={thread_name}\trole={}\tintended_nice=inherit\tactual_nice={}\taffinity=inherit\tallowed_cpus={}\tprocessor={}\tnice_status=skipped\taffinity_status=skipped",
            role.label(),
            actual_nice.map_or_else(|| "unknown".to_string(), |nice| nice.to_string()),
            report.allowed_cpus,
            processor.map_or_else(|| "unknown".to_string(), |cpu| cpu.to_string())
        );
        return report;
    }
    apply_runtime_thread_policy_with(role, resolved_policy(role), &thread_name)
}

pub fn apply_runtime_thread_policy_override(
    role: RuntimeThreadRole,
    policy: RuntimeThreadPolicy,
) -> RuntimeThreadPolicyReport {
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string();
    apply_runtime_thread_policy_with(role, policy, &thread_name)
}

fn apply_runtime_thread_policy_with(
    role: RuntimeThreadRole,
    policy: RuntimeThreadPolicy,
    thread_name: &str,
) -> RuntimeThreadPolicyReport {
    let nice_status = apply_nice(policy.nice);
    let affinity_status = apply_affinity(policy.affinity);
    let scheduler_status = apply_scheduler(policy.scheduler);
    let actual_nice = current_nice();
    let processor = current_processor();
    let report = RuntimeThreadPolicyReport {
        role: role.label(),
        intended_nice: Some(policy.nice),
        actual_nice,
        intended_affinity: policy.affinity.label(),
        allowed_cpus: current_allowed_cpu_list(),
        processor,
        scheduler_policy: current_scheduler_policy(),
        scheduler_priority: current_scheduler_priority(),
        thread_id: current_thread_id(),
        nice_status,
        affinity_status,
        intended_scheduler: policy.scheduler.label(),
        scheduler_status,
    };
    crate::catalog_logln!(
        "thread_policy_tsv\tthread={thread_name}\trole={}\tintended_nice={}\tactual_nice={}\taffinity={}\tallowed_cpus={}\tprocessor={}\tscheduler={}\tscheduler_priority={}\tnice_status={nice_status}\taffinity_status={affinity_status}\tscheduler_status={scheduler_status}",
        role.label(),
        policy.nice,
        actual_nice.map_or_else(|| "unknown".to_string(), |nice| nice.to_string()),
        policy.affinity.label(),
        report.allowed_cpus,
        processor.map_or_else(|| "unknown".to_string(), |cpu| cpu.to_string()),
        policy.scheduler.label(),
        report
            .scheduler_priority
            .map_or_else(|| "unknown".to_string(), |priority| priority.to_string())
    );
    report
}

#[cfg(target_os = "linux")]
fn apply_scheduler(scheduler: ThreadScheduler) -> &'static str {
    match scheduler {
        ThreadScheduler::Other => "skipped",
        ThreadScheduler::RoundRobin { priority } => {
            // SAFETY: sched_setscheduler updates only the current thread using
            // the fully initialized sched_param. Failure remains explicit in
            // the benchmark policy report.
            let parameter = libc::sched_param {
                sched_priority: priority,
            };
            if unsafe { libc::sched_setscheduler(0, libc::SCHED_RR, &parameter) } == 0 {
                "ok"
            } else {
                "failed"
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_scheduler(_scheduler: ThreadScheduler) -> &'static str {
    "unsupported"
}

fn resolved_policy(role: RuntimeThreadRole) -> RuntimeThreadPolicy {
    let mut policy = role.default_policy();
    if affinity_disabled() {
        policy.affinity = ThreadAffinity::Inherit;
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
    env_value_is_off(std::env::var(name).ok().as_deref())
}

fn env_value_is_off(value: Option<&str>) -> bool {
    matches!(value, Some("0" | "off" | "false" | "no" | "any"))
}

#[cfg(target_os = "linux")]
fn apply_nice(nice: i32) -> &'static str {
    // SAFETY: setpriority only adjusts the current thread/process scheduling
    // nice value. Failure is non-fatal; the worker continues at its inherited
    // priority and the status is emitted for benchmarks.
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) };
    if rc == 0 { "ok" } else { "failed" }
}

#[cfg(not(target_os = "linux"))]
fn apply_nice(_nice: i32) -> &'static str {
    "unsupported"
}

#[cfg(target_os = "linux")]
fn apply_affinity(affinity: ThreadAffinity) -> &'static str {
    match affinity {
        ThreadAffinity::Inherit => "skipped",
        ThreadAffinity::AllOnline | ThreadAffinity::Cpu0 | ThreadAffinity::Cpu1 => {
            // SAFETY: cpu_set_t is a plain C bitset. sched_setaffinity with pid
            // 0 targets the current thread on Linux; failure is non-fatal.
            unsafe {
                let mut set: libc::cpu_set_t = std::mem::zeroed();
                libc::CPU_ZERO(&mut set);
                let cpus = match affinity {
                    ThreadAffinity::AllOnline => online_cpu_indices(libc::CPU_SETSIZE as usize),
                    ThreadAffinity::Cpu0 => vec![0],
                    ThreadAffinity::Cpu1 => vec![1],
                    ThreadAffinity::Inherit => unreachable!(),
                };
                for cpu in cpus {
                    libc::CPU_SET(cpu, &mut set);
                }
                let rc = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
                if rc == 0 { "ok" } else { "failed" }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn online_cpu_count() -> usize {
    // SAFETY: sysconf only reads the kernel's online processor count.
    let count = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    usize::try_from(count)
        .ok()
        .filter(|count| *count > 0)
        .unwrap_or(1)
}

#[cfg(target_os = "linux")]
fn online_cpu_indices(capacity: usize) -> Vec<usize> {
    std::fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|online| parse_cpu_list(&online, capacity))
        .filter(|cpus| !cpus.is_empty())
        .unwrap_or_else(|| contiguous_cpu_indices(online_cpu_count(), capacity))
}

#[cfg(any(target_os = "linux", test))]
fn contiguous_cpu_indices(online: usize, capacity: usize) -> Vec<usize> {
    (0..online.max(1).min(capacity)).collect()
}

#[cfg(any(target_os = "linux", test))]
fn parse_cpu_list(value: &str, capacity: usize) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for field in value.trim().split(',') {
        if field.is_empty() {
            return None;
        }
        let (start, end) = if let Some((start, end)) = field.split_once('-') {
            let start = start.parse::<usize>().ok()?;
            let end = end.parse::<usize>().ok()?;
            if start > end {
                return None;
            }
            (start, end)
        } else {
            let cpu = field.parse::<usize>().ok()?;
            (cpu, cpu)
        };
        if start < capacity {
            for cpu in start..=end.min(capacity.saturating_sub(1)) {
                cpus.push(cpu);
            }
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Some(cpus)
}

#[cfg(target_os = "linux")]
fn current_allowed_cpu_list() -> String {
    // SAFETY: sched_getaffinity initializes the provided cpu_set_t for the
    // current thread. Failure is reported as unknown telemetry.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return "unknown".to_string();
        }
        let allowed = (0..libc::CPU_SETSIZE as usize)
            .filter(|cpu| libc::CPU_ISSET(*cpu, &set))
            .collect::<Vec<_>>();
        format_cpu_ranges(&allowed)
    }
}

#[cfg(not(target_os = "linux"))]
fn current_allowed_cpu_list() -> String {
    "unknown".to_string()
}

#[cfg(any(target_os = "linux", test))]
fn format_cpu_ranges(cpus: &[usize]) -> String {
    if cpus.is_empty() {
        return "none".to_string();
    }
    let mut ranges = Vec::new();
    let mut start = cpus[0];
    let mut end = start;
    for &cpu in &cpus[1..] {
        if cpu == end + 1 {
            end = cpu;
            continue;
        }
        ranges.push(format_cpu_range(start, end));
        start = cpu;
        end = cpu;
    }
    ranges.push(format_cpu_range(start, end));
    ranges.join(",")
}

#[cfg(any(target_os = "linux", test))]
fn format_cpu_range(start: usize, end: usize) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
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

#[cfg(target_os = "linux")]
fn current_scheduler_policy() -> Option<i32> {
    // SAFETY: sched_getscheduler only reads the current thread's scheduler class.
    let policy = unsafe { libc::sched_getscheduler(0) };
    (policy >= 0).then_some(policy)
}

#[cfg(not(target_os = "linux"))]
fn current_scheduler_policy() -> Option<i32> {
    None
}

#[cfg(target_os = "linux")]
fn current_scheduler_priority() -> Option<i32> {
    // SAFETY: sched_getparam initializes the provided sched_param for the current thread.
    unsafe {
        let mut parameter: libc::sched_param = std::mem::zeroed();
        (libc::sched_getparam(0, &mut parameter) == 0).then_some(parameter.sched_priority)
    }
}

#[cfg(not(target_os = "linux"))]
fn current_scheduler_priority() -> Option<i32> {
    None
}

#[cfg(target_os = "linux")]
fn current_thread_id() -> Option<i32> {
    // SAFETY: SYS_gettid has no arguments and returns the calling Linux thread ID.
    i32::try_from(unsafe { libc::syscall(libc::SYS_gettid) }).ok()
}

#[cfg(not(target_os = "linux"))]
fn current_thread_id() -> Option<i32> {
    None
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
            RuntimeThreadPolicy::new(0, ThreadAffinity::Inherit)
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
            RuntimeThreadPolicy::new(-10, ThreadAffinity::Cpu1)
        );
    }

    #[test]
    fn heavy_background_roles_default_to_cpu0_affinity() {
        for role in [
            RuntimeThreadRole::CatalogWorker,
            RuntimeThreadRole::SystemEntryPrepare,
            RuntimeThreadRole::InputDiscovery,
            RuntimeThreadRole::LibraryWalker,
            RuntimeThreadRole::MediaWorker,
            RuntimeThreadRole::MediaIndex,
            RuntimeThreadRole::FramebufferStream,
            RuntimeThreadRole::ScreensaverRenderer,
            RuntimeThreadRole::StartupIntroSnapshot,
            RuntimeThreadRole::ScreensaverLoader,
            RuntimeThreadRole::ScreensaverScaler,
        ] {
            assert_eq!(role.default_policy().affinity, ThreadAffinity::Cpu0);
            if matches!(
                role,
                RuntimeThreadRole::ScreensaverRenderer | RuntimeThreadRole::SystemEntryPrepare
            ) {
                assert!(role.default_policy().nice <= 0);
            } else {
                assert!(role.default_policy().nice >= 5);
            }
        }
    }

    #[test]
    fn particle_preparation_uses_the_launcher_core_at_render_priority() {
        assert_eq!(
            RuntimeThreadRole::ParticlePreparer.default_policy(),
            RuntimeThreadPolicy::new(-5, ThreadAffinity::Cpu1)
        );
        assert_eq!(
            RuntimeThreadRole::RuntimeStatus.default_policy(),
            RuntimeThreadPolicy::new(10, ThreadAffinity::Cpu1)
        );
    }

    #[test]
    fn visible_media_download_runs_at_interactive_priority() {
        assert_eq!(
            RuntimeThreadRole::MediaDownload.default_policy(),
            RuntimeThreadPolicy::new(0, ThreadAffinity::AllOnline)
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
                RuntimeThreadPolicy::new(0, ThreadAffinity::AllOnline)
            );
        }
    }

    #[test]
    fn every_role_has_the_expected_production_policy() {
        let expected = [
            (RuntimeThreadRole::LauncherUi, -10, ThreadAffinity::Cpu1),
            (RuntimeThreadRole::InputReader, -15, ThreadAffinity::Cpu1),
            (RuntimeThreadRole::InputDiscovery, 10, ThreadAffinity::Cpu0),
            (RuntimeThreadRole::CatalogWorker, 5, ThreadAffinity::Cpu0),
            (
                RuntimeThreadRole::SystemEntryPrepare,
                0,
                ThreadAffinity::Cpu0,
            ),
            (
                RuntimeThreadRole::CatalogForeground,
                0,
                ThreadAffinity::AllOnline,
            ),
            (RuntimeThreadRole::LibraryWalker, 10, ThreadAffinity::Cpu0),
            (
                RuntimeThreadRole::LibraryWalkerForeground,
                0,
                ThreadAffinity::AllOnline,
            ),
            (
                RuntimeThreadRole::PreviewSelected,
                0,
                ThreadAffinity::Inherit,
            ),
            (RuntimeThreadRole::PreviewPrefetch, 10, ThreadAffinity::Cpu0),
            (RuntimeThreadRole::MediaWorker, 10, ThreadAffinity::Cpu0),
            (
                RuntimeThreadRole::MediaDownload,
                0,
                ThreadAffinity::AllOnline,
            ),
            (RuntimeThreadRole::MediaIndex, 10, ThreadAffinity::Cpu0),
            (
                RuntimeThreadRole::FramebufferStream,
                10,
                ThreadAffinity::Cpu0,
            ),
            (RuntimeThreadRole::RuntimeStatus, 10, ThreadAffinity::Cpu1),
            (RuntimeThreadRole::VideoDecode, 5, ThreadAffinity::Inherit),
            (RuntimeThreadRole::VideoAudio, 5, ThreadAffinity::Inherit),
            (
                RuntimeThreadRole::ScreensaverRenderer,
                -5,
                ThreadAffinity::Cpu0,
            ),
            (
                RuntimeThreadRole::ParticlePreparer,
                -5,
                ThreadAffinity::Cpu1,
            ),
            (
                RuntimeThreadRole::StartupIntroSnapshot,
                10,
                ThreadAffinity::Cpu0,
            ),
            (
                RuntimeThreadRole::ScreensaverLoader,
                10,
                ThreadAffinity::Cpu0,
            ),
            (
                RuntimeThreadRole::ScreensaverScaler,
                10,
                ThreadAffinity::Cpu0,
            ),
        ];
        for (role, nice, affinity) in expected {
            assert_eq!(
                role.default_policy(),
                RuntimeThreadPolicy::new(nice, affinity)
            );
        }
    }

    #[test]
    fn all_online_fallback_is_bounded_by_online_count_and_capacity() {
        assert_eq!(contiguous_cpu_indices(2, 1024), vec![0, 1]);
        assert_eq!(contiguous_cpu_indices(8, 4), vec![0, 1, 2, 3]);
        assert_eq!(contiguous_cpu_indices(0, 1024), vec![0]);
        assert!(contiguous_cpu_indices(2, 0).is_empty());
    }

    #[test]
    fn linux_online_cpu_list_parses_ranges_noncontiguous_ids_and_capacity() {
        assert_eq!(parse_cpu_list("0-1\n", 1024), Some(vec![0, 1]));
        assert_eq!(parse_cpu_list("0,2-3,7\n", 1024), Some(vec![0, 2, 3, 7]));
        assert_eq!(parse_cpu_list("7,1,3,3", 1024), Some(vec![1, 3, 7]));
        assert_eq!(parse_cpu_list("0-7", 4), Some(vec![0, 1, 2, 3]));
        assert_eq!(parse_cpu_list("4-7", 4), Some(Vec::new()));
        assert_eq!(parse_cpu_list("3-1", 1024), None);
        assert_eq!(parse_cpu_list("0,,2", 1024), None);
    }

    #[test]
    fn allowed_cpu_lists_use_linux_range_notation() {
        assert_eq!(format_cpu_ranges(&[]), "none");
        assert_eq!(format_cpu_ranges(&[0]), "0");
        assert_eq!(format_cpu_ranges(&[0, 1]), "0-1");
        assert_eq!(format_cpu_ranges(&[0, 1, 3, 5, 6, 7]), "0-1,3,5-7");
    }

    #[test]
    fn env_off_values_are_recognized() {
        for value in ["0", "off", "false", "no", "any"] {
            assert!(env_value_is_off(Some(value)));
        }
        assert!(!env_value_is_off(Some("on")));
        assert!(!env_value_is_off(None));
    }
}
