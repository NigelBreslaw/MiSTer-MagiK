// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded Linux hardware-counter spans for MiSTer MagiK.
//!
//! The production event set deliberately fits in one Cortex-A9 PMU group so
//! ratios are formed from counters that ran over the same interval. Unsupported
//! platforms and every syscall failure are explicit; missing counters are never
//! represented as successful zero-valued samples.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

const EVENT_SET: [HardwareEvent; 6] = [
    HardwareEvent::Cycles,
    HardwareEvent::Instructions,
    HardwareEvent::L1dAccesses,
    HardwareEvent::L1dRefills,
    HardwareEvent::Branches,
    HardwareEvent::BranchMispredicts,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HardwareEvent {
    Cycles,
    Instructions,
    L1dAccesses,
    L1dRefills,
    Branches,
    BranchMispredicts,
}

impl HardwareEvent {
    #[must_use]
    pub const fn perf_config(self) -> u64 {
        match self {
            Self::Cycles => 0,
            Self::Instructions => 1,
            Self::L1dAccesses => 2,
            Self::L1dRefills => 3,
            Self::Branches => 4,
            Self::BranchMispredicts => 5,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cycles => "cycles",
            Self::Instructions => "instructions",
            Self::L1dAccesses => "l1d-accesses",
            Self::L1dRefills => "l1d-refills",
            Self::Branches => "branches",
            Self::BranchMispredicts => "branch-mispredicts",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EventMetadata {
    pub event: HardwareEvent,
    pub perf_type: u32,
    pub perf_config: u64,
    pub semantic: &'static str,
}

#[must_use]
pub const fn event_metadata() -> [EventMetadata; EVENT_SET.len()] {
    [
        EventMetadata {
            event: HardwareEvent::Cycles,
            perf_type: 0,
            perf_config: 0,
            semantic: "CPU cycles",
        },
        EventMetadata {
            event: HardwareEvent::Instructions,
            perf_type: 0,
            perf_config: 1,
            semantic: "instructions",
        },
        EventMetadata {
            event: HardwareEvent::L1dAccesses,
            perf_type: 0,
            perf_config: 2,
            semantic: "Cortex-A9 L1 data-cache accesses",
        },
        EventMetadata {
            event: HardwareEvent::L1dRefills,
            perf_type: 0,
            perf_config: 3,
            semantic: "Cortex-A9 L1 data-cache refills",
        },
        EventMetadata {
            event: HardwareEvent::Branches,
            perf_type: 0,
            perf_config: 4,
            semantic: "branches",
        },
        EventMetadata {
            event: HardwareEvent::BranchMispredicts,
            perf_type: 0,
            perf_config: 5,
            semantic: "branch mispredicts",
        },
    ]
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PmuFailure {
    pub stage: String,
    pub event: Option<HardwareEvent>,
    pub errno: Option<i32>,
    pub message: String,
}

impl PmuFailure {
    #[cfg(any(not(target_os = "linux"), test))]
    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            stage: "unsupported".to_owned(),
            event: None,
            errno: None,
            message: message.into(),
        }
    }

    #[cfg(target_os = "linux")]
    fn io(stage: &'static str, event: Option<HardwareEvent>, error: std::io::Error) -> Self {
        Self {
            stage: stage.to_owned(),
            event,
            errno: error.raw_os_error(),
            message: error.to_string(),
        }
    }

    fn malformed(message: impl Into<String>) -> Self {
        Self {
            stage: "decode-group-read".to_owned(),
            event: None,
            errno: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for PmuFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "PMU {}", self.stage)?;
        if let Some(event) = self.event {
            write!(formatter, " for {}", event.label())?;
        }
        if let Some(errno) = self.errno {
            write!(formatter, " (errno {errno})")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl std::error::Error for PmuFailure {}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CounterValues {
    pub cycles: u64,
    pub instructions: u64,
    pub l1d_accesses: u64,
    pub l1d_refills: u64,
    pub branches: u64,
    pub branch_mispredicts: u64,
}

impl CounterValues {
    fn set(&mut self, event: HardwareEvent, value: u64) {
        match event {
            HardwareEvent::Cycles => self.cycles = value,
            HardwareEvent::Instructions => self.instructions = value,
            HardwareEvent::L1dAccesses => self.l1d_accesses = value,
            HardwareEvent::L1dRefills => self.l1d_refills = value,
            HardwareEvent::Branches => self.branches = value,
            HardwareEvent::BranchMispredicts => self.branch_mispredicts = value,
        }
    }

    fn saturating_sub(self, earlier: Self) -> Self {
        Self {
            cycles: self.cycles.saturating_sub(earlier.cycles),
            instructions: self.instructions.saturating_sub(earlier.instructions),
            l1d_accesses: self.l1d_accesses.saturating_sub(earlier.l1d_accesses),
            l1d_refills: self.l1d_refills.saturating_sub(earlier.l1d_refills),
            branches: self.branches.saturating_sub(earlier.branches),
            branch_mispredicts: self
                .branch_mispredicts
                .saturating_sub(earlier.branch_mispredicts),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CounterSnapshot {
    pub time_enabled_ns: u64,
    pub time_running_ns: u64,
    pub counters: CounterValues,
}

impl CounterSnapshot {
    #[must_use]
    pub fn delta_from(self, earlier: Self) -> CounterDelta {
        CounterDelta {
            time_enabled_ns: self
                .time_enabled_ns
                .saturating_sub(earlier.time_enabled_ns),
            time_running_ns: self
                .time_running_ns
                .saturating_sub(earlier.time_running_ns),
            counters: self.counters.saturating_sub(earlier.counters),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CounterDelta {
    pub time_enabled_ns: u64,
    pub time_running_ns: u64,
    pub counters: CounterValues,
}

impl CounterDelta {
    #[must_use]
    pub fn instructions_per_cycle(self) -> f64 {
        ratio(self.counters.instructions, self.counters.cycles)
    }

    #[must_use]
    pub fn cycles_per_instruction(self) -> f64 {
        ratio(self.counters.cycles, self.counters.instructions)
    }

    #[must_use]
    pub fn l1d_refill_percent(self) -> f64 {
        ratio(self.counters.l1d_refills, self.counters.l1d_accesses) * 100.0
    }

    #[must_use]
    pub fn branch_mispredict_percent(self) -> f64 {
        ratio(self.counters.branch_mispredicts, self.counters.branches) * 100.0
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpanRecord {
    pub name: String,
    pub counters: CounterDelta,
}

const DEFAULT_SAMPLE_EVERY: u64 = 16;
const DEFAULT_RECORD_LIMIT: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThreadProfile {
    pub schema: &'static str,
    pub enabled: bool,
    pub sample_every: u64,
    pub attempted_spans: u64,
    pub dropped_spans: u64,
    pub records: Vec<SpanRecord>,
    pub failure: Option<PmuFailure>,
}

struct ThreadCollector {
    enabled: bool,
    sample_every: u64,
    record_limit: usize,
    attempted_spans: u64,
    dropped_spans: u64,
    calls_by_name: BTreeMap<&'static str, u64>,
    records: Vec<SpanRecord>,
    failure: Option<PmuFailure>,
    group: Option<CounterGroup>,
}

impl ThreadCollector {
    fn from_env() -> Self {
        let enabled = std::env::var_os("MISTER_PMU_PROFILE").is_some_and(|value| value == "1");
        let sample_every = bounded_env_u64("MISTER_PMU_SAMPLE_EVERY", DEFAULT_SAMPLE_EVERY, 1, 10_000);
        let record_limit = bounded_env_u64(
            "MISTER_PMU_RECORD_LIMIT",
            DEFAULT_RECORD_LIMIT as u64,
            1,
            65_536,
        ) as usize;
        Self::new(enabled, sample_every, record_limit)
    }

    fn new(enabled: bool, sample_every: u64, record_limit: usize) -> Self {
        Self {
            enabled,
            sample_every,
            record_limit,
            attempted_spans: 0,
            dropped_spans: 0,
            calls_by_name: BTreeMap::new(),
            records: Vec::new(),
            failure: None,
            group: None,
        }
    }

    fn start(&mut self, name: &'static str) -> Option<CounterSnapshot> {
        if !self.enabled || self.failure.is_some() {
            return None;
        }
        self.attempted_spans = self.attempted_spans.saturating_add(1);
        let calls = self.calls_by_name.entry(name).or_default();
        *calls = calls.saturating_add(1);
        if !(*calls - 1).is_multiple_of(self.sample_every) {
            return None;
        }
        if self.group.is_none() {
            match CounterGroup::open() {
                Ok(group) => self.group = Some(group),
                Err(failure) => {
                    self.failure = Some(failure);
                    return None;
                }
            }
        }
        match self.group.as_ref().expect("PMU group initialized").snapshot() {
            Ok(snapshot) => Some(snapshot),
            Err(failure) => {
                self.failure = Some(failure);
                self.group = None;
                None
            }
        }
    }

    fn finish(&mut self, name: &'static str, started: CounterSnapshot) {
        let Some(group) = self.group.as_ref() else {
            return;
        };
        match group.snapshot() {
            Ok(finished) if self.records.len() < self.record_limit => self.records.push(SpanRecord {
                name: name.to_owned(),
                counters: finished.delta_from(started),
            }),
            Ok(_) => self.dropped_spans = self.dropped_spans.saturating_add(1),
            Err(failure) => {
                self.failure = Some(failure);
                self.group = None;
            }
        }
    }

    fn take(&mut self) -> ThreadProfile {
        let profile = ThreadProfile {
            schema: "mister-magik-pmu-thread-profile-v1",
            enabled: self.enabled,
            sample_every: self.sample_every,
            attempted_spans: self.attempted_spans,
            dropped_spans: self.dropped_spans,
            records: std::mem::take(&mut self.records),
            failure: self.failure.clone(),
        };
        self.attempted_spans = 0;
        self.dropped_spans = 0;
        self.calls_by_name.clear();
        profile
    }
}

fn bounded_env_u64(name: &str, default: u64, minimum: u64, maximum: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .map_or(default, |value: u64| value.clamp(minimum, maximum))
}

thread_local! {
    static THREAD_COLLECTOR: RefCell<ThreadCollector> = RefCell::new(ThreadCollector::from_env());
}

/// Starts a sampled span on the calling thread when `MISTER_PMU_PROFILE=1`.
///
/// Sampling is independent for each span name, preventing a fixed phase order
/// from starving all but one phase. Dropping the guard records the end sample
/// in thread-local memory; it never performs file I/O.
#[must_use]
pub fn sampled_span(name: &'static str) -> Option<SampledSpan> {
    let started = THREAD_COLLECTOR.with(|collector| collector.borrow_mut().start(name))?;
    Some(SampledSpan {
        name,
        started,
        finished: false,
        not_send: PhantomData,
    })
}

/// Removes the accumulated records for the calling thread.
#[must_use]
pub fn take_thread_profile() -> ThreadProfile {
    THREAD_COLLECTOR.with(|collector| collector.borrow_mut().take())
}

pub struct SampledSpan {
    name: &'static str,
    started: CounterSnapshot,
    finished: bool,
    not_send: PhantomData<Rc<()>>,
}

impl SampledSpan {
    pub fn finish(mut self) {
        self.finish_inner();
    }

    fn finish_inner(&mut self) {
        if self.finished {
            return;
        }
        THREAD_COLLECTOR.with(|collector| {
            collector.borrow_mut().finish(self.name, self.started);
        });
        self.finished = true;
    }
}

impl Drop for SampledSpan {
    fn drop(&mut self) {
        self.finish_inner();
    }
}

pub struct NamedSpan<'a> {
    group: &'a CounterGroup,
    name: String,
    started: CounterSnapshot,
}

impl NamedSpan<'_> {
    pub fn finish(self) -> Result<SpanRecord, PmuFailure> {
        let finished = self.group.snapshot()?;
        Ok(SpanRecord {
            name: self.name,
            counters: finished.delta_from(self.started),
        })
    }
}

pub struct CounterGroup {
    #[cfg(target_os = "linux")]
    inner: linux::LinuxCounterGroup,
}

impl CounterGroup {
    pub fn open() -> Result<Self, PmuFailure> {
        #[cfg(target_os = "linux")]
        {
            linux::LinuxCounterGroup::open().map(|inner| Self { inner })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(PmuFailure::unsupported(
                "Linux perf_event_open is unavailable on this platform",
            ))
        }
    }

    pub fn snapshot(&self) -> Result<CounterSnapshot, PmuFailure> {
        #[cfg(target_os = "linux")]
        {
            self.inner.snapshot()
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(PmuFailure::unsupported(
                "Linux perf_event_open is unavailable on this platform",
            ))
        }
    }

    pub fn span(&self, name: impl Into<String>) -> Result<NamedSpan<'_>, PmuFailure> {
        let started = self.snapshot()?;
        Ok(NamedSpan {
            group: self,
            name: name.into(),
            started,
        })
    }
}

fn decode_group_read(
    words: &[u64],
    event_ids: &BTreeMap<u64, HardwareEvent>,
) -> Result<CounterSnapshot, PmuFailure> {
    let expected_words = 3 + event_ids.len() * 2;
    if words.len() != expected_words {
        return Err(PmuFailure::malformed(format!(
            "group read contained {} words, expected {expected_words}",
            words.len()
        )));
    }
    if words[0] != event_ids.len() as u64 {
        return Err(PmuFailure::malformed(format!(
            "group read reported {} events, expected {}",
            words[0],
            event_ids.len()
        )));
    }
    let mut counters = CounterValues::default();
    let mut seen = BTreeMap::new();
    for pair in words[3..].chunks_exact(2) {
        let value = pair[0];
        let id = pair[1];
        let event = event_ids
            .get(&id)
            .copied()
            .ok_or_else(|| PmuFailure::malformed(format!("unknown event id {id}")))?;
        if seen.insert(event, ()).is_some() {
            return Err(PmuFailure::malformed(format!(
                "duplicate event id for {}",
                event.label()
            )));
        }
        counters.set(event, value);
    }
    if seen.len() != event_ids.len() {
        return Err(PmuFailure::malformed("group read omitted an event"));
    }
    Ok(CounterSnapshot {
        time_enabled_ns: words[1],
        time_running_ns: words[2],
        counters,
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{CounterSnapshot, EVENT_SET, HardwareEvent, PmuFailure, decode_group_read};
    use std::collections::BTreeMap;

    const PERF_TYPE_HARDWARE: u32 = 0;
    const PERF_FORMAT_TOTAL_TIME_ENABLED: u64 = 1;
    const PERF_FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;
    const PERF_FORMAT_ID: u64 = 1 << 2;
    const PERF_FORMAT_GROUP: u64 = 1 << 3;
    const PERF_ATTR_DISABLED: u64 = 1;
    const PERF_ATTR_EXCLUDE_KERNEL: u64 = 1 << 5;
    const PERF_ATTR_EXCLUDE_HYPERVISOR: u64 = 1 << 6;
    const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 1 << 3;
    const PERF_EVENT_IOC_ENABLE: libc::c_ulong = 0x2400;
    const PERF_EVENT_IOC_DISABLE: libc::c_ulong = 0x2401;
    const PERF_EVENT_IOC_RESET: libc::c_ulong = 0x2403;
    const PERF_EVENT_IOC_ID: libc::c_ulong = 0x8008_2407;
    const PERF_IOC_FLAG_GROUP: libc::c_ulong = 1;
    const GROUP_READ_WORDS: usize = 3 + EVENT_SET.len() * 2;

    #[repr(C)]
    #[derive(Default)]
    struct PerfEventAttr {
        event_type: u32,
        size: u32,
        config: u64,
        sample_period: u64,
        sample_type: u64,
        read_format: u64,
        flags: u64,
        wakeup_events: u32,
        breakpoint_type: u32,
        config1: u64,
    }

    struct Descriptor {
        fd: libc::c_int,
    }

    pub(super) struct LinuxCounterGroup {
        descriptors: Vec<Descriptor>,
        event_ids: BTreeMap<u64, HardwareEvent>,
    }

    impl LinuxCounterGroup {
        pub(super) fn open() -> Result<Self, PmuFailure> {
            debug_assert_eq!(std::mem::size_of::<PerfEventAttr>(), 64);
            let leader = open_event(HardwareEvent::Cycles, -1)?;
            let mut group = Self {
                descriptors: vec![Descriptor { fd: leader }],
                event_ids: BTreeMap::new(),
            };
            for event in EVENT_SET.iter().copied().skip(1) {
                let descriptor = open_event(event, leader)?;
                group.descriptors.push(Descriptor { fd: descriptor });
            }
            for (descriptor, event) in group.descriptors.iter().zip(EVENT_SET) {
                let id = event_id(descriptor.fd, event)?;
                if group.event_ids.insert(id, event).is_some() {
                    return Err(PmuFailure::malformed(format!(
                        "duplicate kernel event id {id}"
                    )));
                }
            }
            group.ioctl_group(PERF_EVENT_IOC_RESET, "reset-group")?;
            group.ioctl_group(PERF_EVENT_IOC_ENABLE, "enable-group")?;
            Ok(group)
        }

        pub(super) fn snapshot(&self) -> Result<CounterSnapshot, PmuFailure> {
            let mut words = [0_u64; GROUP_READ_WORDS];
            let expected_bytes = std::mem::size_of_val(&words);
            // SAFETY: `words` is writable for exactly `expected_bytes`, and the
            // leader descriptor remains owned by this group for the call.
            let read_bytes = unsafe {
                libc::read(
                    self.descriptors[0].fd,
                    words.as_mut_ptr().cast(),
                    expected_bytes,
                )
            };
            if read_bytes < 0 {
                return Err(PmuFailure::io(
                    "read-group",
                    None,
                    std::io::Error::last_os_error(),
                ));
            }
            if read_bytes as usize != expected_bytes {
                return Err(PmuFailure::malformed(format!(
                    "group read returned {read_bytes} bytes, expected {expected_bytes}"
                )));
            }
            decode_group_read(&words, &self.event_ids)
        }

        fn ioctl_group(
            &self,
            request: libc::c_ulong,
            stage: &'static str,
        ) -> Result<(), PmuFailure> {
            // SAFETY: the descriptor is the live group leader, and the request
            // takes the documented integer group flag rather than a pointer.
            if unsafe { libc::ioctl(self.descriptors[0].fd, request, PERF_IOC_FLAG_GROUP) } == 0 {
                Ok(())
            } else {
                Err(PmuFailure::io(
                    stage,
                    None,
                    std::io::Error::last_os_error(),
                ))
            }
        }
    }

    impl Drop for LinuxCounterGroup {
        fn drop(&mut self) {
            if let Some(leader) = self.descriptors.first() {
                // SAFETY: the leader remains live here; failure during cleanup
                // cannot be recovered and closing the descriptors still follows.
                unsafe {
                    libc::ioctl(leader.fd, PERF_EVENT_IOC_DISABLE, PERF_IOC_FLAG_GROUP);
                }
            }
            for descriptor in self.descriptors.drain(..) {
                // SAFETY: each descriptor is uniquely owned by the group.
                unsafe {
                    libc::close(descriptor.fd);
                }
            }
        }
    }

    fn open_event(
        event: HardwareEvent,
        group_descriptor: libc::c_int,
    ) -> Result<libc::c_int, PmuFailure> {
        let attributes = PerfEventAttr {
            event_type: PERF_TYPE_HARDWARE,
            size: 64,
            config: event.perf_config(),
            read_format: PERF_FORMAT_TOTAL_TIME_ENABLED
                | PERF_FORMAT_TOTAL_TIME_RUNNING
                | PERF_FORMAT_ID
                | PERF_FORMAT_GROUP,
            flags: PERF_ATTR_DISABLED | PERF_ATTR_EXCLUDE_KERNEL | PERF_ATTR_EXCLUDE_HYPERVISOR,
            ..PerfEventAttr::default()
        };
        // SAFETY: the syscall receives a valid version-zero attribute structure
        // and requests counters for the calling thread on any CPU.
        let raw = unsafe {
            libc::syscall(
                libc::SYS_perf_event_open,
                &attributes,
                0,
                -1,
                group_descriptor,
                PERF_FLAG_FD_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(PmuFailure::io(
                "open-event",
                Some(event),
                std::io::Error::last_os_error(),
            ));
        }
        libc::c_int::try_from(raw).map_err(|_| {
            PmuFailure::malformed(format!("descriptor for {} exceeds c_int", event.label()))
        })
    }

    fn event_id(fd: libc::c_int, event: HardwareEvent) -> Result<u64, PmuFailure> {
        let mut id = 0_u64;
        // SAFETY: `id` is writable and the descriptor is a live perf event.
        if unsafe { libc::ioctl(fd, PERF_EVENT_IOC_ID, &mut id) } == 0 {
            Ok(id)
        } else {
            Err(PmuFailure::io(
                "read-event-id",
                Some(event),
                std::io::Error::last_os_error(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> BTreeMap<u64, HardwareEvent> {
        EVENT_SET
            .into_iter()
            .enumerate()
            .map(|(index, event)| (100 + index as u64, event))
            .collect()
    }

    #[test]
    fn group_read_is_mapped_by_event_id() {
        let words = [
            6, 1_000, 900, 30, 102, 60, 100, 3, 105, 20, 104, 45, 101, 4, 103,
        ];
        let snapshot = decode_group_read(&words, &ids()).unwrap();
        assert_eq!(snapshot.time_enabled_ns, 1_000);
        assert_eq!(snapshot.time_running_ns, 900);
        assert_eq!(snapshot.counters.cycles, 60);
        assert_eq!(snapshot.counters.instructions, 45);
        assert_eq!(snapshot.counters.l1d_accesses, 30);
        assert_eq!(snapshot.counters.l1d_refills, 4);
        assert_eq!(snapshot.counters.branches, 20);
        assert_eq!(snapshot.counters.branch_mispredicts, 3);
    }

    #[test]
    fn malformed_group_reads_are_rejected() {
        assert!(decode_group_read(&[6, 1, 1], &ids()).is_err());
        let mut wrong_count = [0_u64; 15];
        wrong_count[0] = 5;
        assert!(decode_group_read(&wrong_count, &ids()).is_err());
        let unknown = [6, 1, 1, 1, 999, 1, 100, 1, 101, 1, 102, 1, 103, 1, 104];
        assert!(decode_group_read(&unknown, &ids()).is_err());
    }

    #[test]
    fn counter_deltas_saturate_and_ratios_are_bounded_by_zero_denominators() {
        let earlier = CounterSnapshot {
            time_enabled_ns: 100,
            time_running_ns: 80,
            counters: CounterValues {
                cycles: 100,
                instructions: 60,
                l1d_accesses: 20,
                l1d_refills: 5,
                branches: 10,
                branch_mispredicts: 2,
            },
        };
        let later = CounterSnapshot {
            time_enabled_ns: 90,
            time_running_ns: 90,
            counters: CounterValues {
                cycles: 160,
                instructions: 105,
                l1d_accesses: 30,
                l1d_refills: 7,
                branches: 15,
                branch_mispredicts: 3,
            },
        };
        let delta = later.delta_from(earlier);
        assert_eq!(delta.time_enabled_ns, 0);
        assert_eq!(delta.time_running_ns, 10);
        assert_eq!(delta.counters.cycles, 60);
        assert_eq!(delta.instructions_per_cycle(), 0.75);
        assert_eq!(delta.l1d_refill_percent(), 20.0);
        assert_eq!(CounterDelta::default().cycles_per_instruction(), 0.0);
    }

    #[test]
    fn overlapping_spans_form_independent_nested_deltas() {
        let outer_start = CounterSnapshot::default();
        let inner_start = CounterSnapshot {
            time_enabled_ns: 10,
            time_running_ns: 8,
            counters: CounterValues {
                cycles: 100,
                instructions: 70,
                ..CounterValues::default()
            },
        };
        let inner_end = CounterSnapshot {
            time_enabled_ns: 20,
            time_running_ns: 16,
            counters: CounterValues {
                cycles: 180,
                instructions: 130,
                ..CounterValues::default()
            },
        };
        let outer_end = CounterSnapshot {
            time_enabled_ns: 30,
            time_running_ns: 24,
            counters: CounterValues {
                cycles: 240,
                instructions: 170,
                ..CounterValues::default()
            },
        };
        assert_eq!(
            inner_end.delta_from(inner_start).counters.cycles,
            80
        );
        assert_eq!(outer_end.delta_from(outer_start).counters.cycles, 240);
    }

    #[test]
    fn failures_and_spans_have_stable_json_fields() {
        let failure = PmuFailure::unsupported("not Linux");
        let value = serde_json::to_value(&failure).unwrap();
        assert_eq!(value["stage"], "unsupported");
        assert_eq!(value["errno"], serde_json::Value::Null);

        let span = SpanRecord {
            name: "outer".to_owned(),
            counters: CounterDelta::default(),
        };
        let value = serde_json::to_value(span).unwrap();
        assert_eq!(value["name"], "outer");
        assert!(value["counters"]["counters"]["cycles"].is_number());
    }

    #[test]
    fn event_metadata_declares_the_complete_non_multiplexed_set() {
        let metadata = event_metadata();
        assert_eq!(metadata.len(), 6);
        assert_eq!(metadata[2].semantic, "Cortex-A9 L1 data-cache accesses");
        assert_eq!(metadata[3].semantic, "Cortex-A9 L1 data-cache refills");
    }

    #[test]
    fn collector_samples_each_name_independently_and_drains_records() {
        let mut collector = ThreadCollector::new(true, 2, 1);
        collector.group = None;
        assert_eq!(collector.sample_every, 2);
        assert_eq!(collector.record_limit, 1);
        let first = collector.calls_by_name.entry("first").or_default();
        *first += 1;
        let second = collector.calls_by_name.entry("second").or_default();
        *second += 1;
        assert_eq!(collector.calls_by_name["first"], 1);
        assert_eq!(collector.calls_by_name["second"], 1);
        let profile = collector.take();
        assert!(profile.enabled);
        assert_eq!(profile.schema, "mister-magik-pmu-thread-profile-v1");
        assert!(collector.calls_by_name.is_empty());
    }
}
