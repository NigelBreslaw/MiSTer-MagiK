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
use std::sync::{Mutex, MutexGuard, OnceLock};

const GENERAL_EVENTS: [HardwareEvent; 6] = [
    HardwareEvent::Cycles,
    HardwareEvent::Instructions,
    HardwareEvent::L1dAccesses,
    HardwareEvent::L1dRefills,
    HardwareEvent::Branches,
    HardwareEvent::BranchMispredicts,
];

const CORTEX_A9_NEON_EVENTS: [HardwareEvent; 7] = [
    HardwareEvent::Cycles,
    HardwareEvent::SpeculativeInstructions,
    HardwareEvent::NeonInstructions,
    HardwareEvent::NeonClockCycles,
    HardwareEvent::DataDependentStallCycles,
    HardwareEvent::L1dAccesses,
    HardwareEvent::L1dRefills,
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CounterSet {
    #[default]
    General,
    CortexA9Neon,
}

impl CounterSet {
    #[must_use]
    pub const fn events(self) -> &'static [HardwareEvent] {
        match self {
            Self::General => &GENERAL_EVENTS,
            Self::CortexA9Neon => &CORTEX_A9_NEON_EVENTS,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HardwareEvent {
    Cycles,
    Instructions,
    L1dAccesses,
    L1dRefills,
    Branches,
    BranchMispredicts,
    SpeculativeInstructions,
    NeonInstructions,
    NeonClockCycles,
    DataDependentStallCycles,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroupReadFormat {
    IdsAndTimes,
    OrderedValues,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CounterScope {
    CallingThreadAnyCpu,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PmuOpenAttempt {
    pub name: String,
    pub read_format: Option<GroupReadFormat>,
    pub scope: CounterScope,
    pub cpu: i32,
    pub perf_flags: u64,
    pub disabled: bool,
    pub exclude_kernel: bool,
    pub exclude_hypervisor: bool,
    pub grouped: bool,
    pub success: bool,
    pub failure: Option<PmuFailure>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PmuEnvironment {
    pub event_sources: Vec<String>,
    pub perf_event_paranoid: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PmuOpenDiagnostics {
    pub environment: PmuEnvironment,
    pub attempts: Vec<PmuOpenAttempt>,
}

impl HardwareEvent {
    #[must_use]
    pub const fn perf_type(self) -> u32 {
        match self {
            Self::SpeculativeInstructions
            | Self::NeonInstructions
            | Self::NeonClockCycles
            | Self::DataDependentStallCycles => 4,
            _ => 0,
        }
    }

    #[must_use]
    pub const fn perf_config(self) -> u64 {
        match self {
            Self::Cycles => 0,
            Self::Instructions => 1,
            Self::L1dAccesses => 2,
            Self::L1dRefills => 3,
            Self::Branches => 4,
            Self::BranchMispredicts => 5,
            Self::SpeculativeInstructions => 0x68,
            Self::NeonInstructions => 0x74,
            Self::NeonClockCycles => 0x8c,
            Self::DataDependentStallCycles => 0x61,
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
            Self::SpeculativeInstructions => "speculative-instructions",
            Self::NeonInstructions => "neon-instructions",
            Self::NeonClockCycles => "neon-clock-cycles",
            Self::DataDependentStallCycles => "data-dependent-stall-cycles",
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
pub const fn event_metadata() -> [EventMetadata; GENERAL_EVENTS.len()] {
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

#[must_use]
pub fn event_metadata_for(counter_set: CounterSet) -> Vec<EventMetadata> {
    counter_set
        .events()
        .iter()
        .copied()
        .map(|event| EventMetadata {
            event,
            perf_type: event.perf_type(),
            perf_config: event.perf_config(),
            semantic: match event {
                HardwareEvent::Cycles => "CPU cycles",
                HardwareEvent::Instructions => "instructions",
                HardwareEvent::L1dAccesses => "Cortex-A9 L1 data-cache accesses",
                HardwareEvent::L1dRefills => "Cortex-A9 L1 data-cache refills",
                HardwareEvent::Branches => "branches",
                HardwareEvent::BranchMispredicts => "branch mispredicts",
                HardwareEvent::SpeculativeInstructions => {
                    "Cortex-A9 instructions speculatively executed"
                }
                HardwareEvent::NeonInstructions => "Cortex-A9 NEON instructions",
                HardwareEvent::NeonClockCycles => "Cortex-A9 cycles with the NEON clock enabled",
                HardwareEvent::DataDependentStallCycles => {
                    "Cortex-A9 cycles stalled on a data dependency"
                }
            },
        })
        .collect()
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

    #[cfg(any(target_os = "linux", test))]
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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CounterValues(BTreeMap<HardwareEvent, u64>);

impl CounterValues {
    #[cfg(any(target_os = "linux", test))]
    fn set(&mut self, event: HardwareEvent, value: u64) {
        self.0.insert(event, value);
    }

    #[must_use]
    pub fn get(&self, event: HardwareEvent) -> Option<u64> {
        self.0.get(&event).copied()
    }

    #[must_use]
    pub fn iter(&self) -> impl Iterator<Item = (HardwareEvent, u64)> + '_ {
        self.0.iter().map(|(event, value)| (*event, *value))
    }

    fn saturating_sub(&self, earlier: &Self) -> Self {
        let mut values = BTreeMap::new();
        for (event, value) in &self.0 {
            if let Some(earlier) = earlier.0.get(event) {
                values.insert(*event, value.saturating_sub(*earlier));
            }
        }
        Self(values)
    }
}

impl<const N: usize> From<[(HardwareEvent, u64); N]> for CounterValues {
    fn from(values: [(HardwareEvent, u64); N]) -> Self {
        Self(values.into_iter().collect())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CounterSnapshot {
    #[serde(default)]
    pub counter_set: CounterSet,
    pub time_enabled_ns: u64,
    pub time_running_ns: u64,
    pub counters: CounterValues,
}

impl CounterSnapshot {
    #[must_use]
    pub fn delta_from(self, earlier: Self) -> CounterDelta {
        CounterDelta {
            counter_set: self.counter_set,
            time_enabled_ns: self.time_enabled_ns.saturating_sub(earlier.time_enabled_ns),
            time_running_ns: self.time_running_ns.saturating_sub(earlier.time_running_ns),
            counters: self.counters.saturating_sub(&earlier.counters),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CounterDelta {
    #[serde(default)]
    pub counter_set: CounterSet,
    pub time_enabled_ns: u64,
    pub time_running_ns: u64,
    pub counters: CounterValues,
}

impl CounterDelta {
    #[must_use]
    pub fn instructions_per_cycle(&self) -> f64 {
        self.ratio(HardwareEvent::Instructions, HardwareEvent::Cycles)
    }

    #[must_use]
    pub fn cycles_per_instruction(&self) -> f64 {
        self.ratio(HardwareEvent::Cycles, HardwareEvent::Instructions)
    }

    #[must_use]
    pub fn l1d_refill_percent(&self) -> f64 {
        self.ratio(HardwareEvent::L1dRefills, HardwareEvent::L1dAccesses) * 100.0
    }

    #[must_use]
    pub fn branch_mispredict_percent(&self) -> f64 {
        self.ratio(HardwareEvent::BranchMispredicts, HardwareEvent::Branches) * 100.0
    }

    #[must_use]
    pub fn ratio(&self, numerator: HardwareEvent, denominator: HardwareEvent) -> f64 {
        match (self.counters.get(numerator), self.counters.get(denominator)) {
            (Some(numerator), Some(denominator)) => ratio(numerator, denominator),
            _ => 0.0,
        }
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
const PROCESS_PROFILE_LIMIT: usize = 16;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThreadProfile {
    pub schema: &'static str,
    pub enabled: bool,
    pub sample_every: u64,
    pub attempted_spans: u64,
    pub dropped_spans: u64,
    pub records: Vec<SpanRecord>,
    pub failure: Option<PmuFailure>,
    pub read_format: Option<GroupReadFormat>,
    pub scope: Option<CounterScope>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SubmittedThreadProfile {
    pub label: String,
    pub profile: ThreadProfile,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProcessProfileBatch {
    pub profiles: Vec<SubmittedThreadProfile>,
    pub dropped_profiles: u64,
}

#[derive(Default)]
struct ProcessCollector {
    profiles: Vec<SubmittedThreadProfile>,
    dropped_profiles: u64,
}

impl ProcessCollector {
    fn submit(&mut self, label: &'static str, profile: ThreadProfile) {
        if !profile.enabled {
            return;
        }
        if self.profiles.len() < PROCESS_PROFILE_LIMIT {
            self.profiles.push(SubmittedThreadProfile {
                label: label.to_owned(),
                profile,
            });
        } else {
            self.dropped_profiles = self.dropped_profiles.saturating_add(1);
        }
    }

    fn take(&mut self) -> ProcessProfileBatch {
        ProcessProfileBatch {
            profiles: std::mem::take(&mut self.profiles),
            dropped_profiles: std::mem::take(&mut self.dropped_profiles),
        }
    }

    fn clear(&mut self) {
        self.profiles.clear();
        self.dropped_profiles = 0;
    }
}

static PROCESS_COLLECTOR: OnceLock<Mutex<ProcessCollector>> = OnceLock::new();
static PROCESS_CONFIG: OnceLock<PmuProfileConfig> = OnceLock::new();

const PMU_PROFILE: &str = "MISTER_PMU_PROFILE";
const PMU_SAMPLE_EVERY: &str = "MISTER_PMU_SAMPLE_EVERY";
const PMU_RECORD_LIMIT: &str = "MISTER_PMU_RECORD_LIMIT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmuProfileConfig {
    enabled: bool,
    sample_every: u64,
    record_limit: usize,
}

impl PmuProfileConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self {
            enabled: get(PMU_PROFILE) == Some("1"),
            sample_every: bounded_value(get(PMU_SAMPLE_EVERY), DEFAULT_SAMPLE_EVERY, 1, 10_000),
            record_limit: bounded_value(
                get(PMU_RECORD_LIMIT),
                DEFAULT_RECORD_LIMIT as u64,
                1,
                65_536,
            ) as usize,
        }
    }
}

pub fn install_process_config(config: PmuProfileConfig) -> Result<(), &'static str> {
    PROCESS_CONFIG
        .set(config)
        .map_err(|_| "PMU process configuration was already installed")
}

fn process_collector() -> MutexGuard<'static, ProcessCollector> {
    PROCESS_COLLECTOR
        .get_or_init(|| Mutex::new(ProcessCollector::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    read_format: Option<GroupReadFormat>,
    scope: Option<CounterScope>,
}

impl ThreadCollector {
    fn from_process_config() -> Self {
        let config = PROCESS_CONFIG.get().copied().unwrap_or_else(|| {
            let values: std::collections::BTreeMap<String, String> = std::env::vars().collect();
            PmuProfileConfig::capture_with(|name| values.get(name).map(String::as_str))
        });
        Self::new(config.enabled, config.sample_every, config.record_limit)
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
            read_format: None,
            scope: None,
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
                Ok(group) => {
                    self.read_format = Some(group.read_format());
                    self.scope = Some(group.scope());
                    self.group = Some(group);
                }
                Err(failure) => {
                    self.failure = Some(failure);
                    return None;
                }
            }
        }
        match self
            .group
            .as_ref()
            .expect("PMU group initialized")
            .snapshot()
        {
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
            Ok(finished) if self.records.len() < self.record_limit => {
                self.records.push(SpanRecord {
                    name: name.to_owned(),
                    counters: finished.delta_from(started),
                })
            }
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
            read_format: self.read_format,
            scope: self.scope,
        };
        self.attempted_spans = 0;
        self.dropped_spans = 0;
        self.calls_by_name.clear();
        profile
    }
}

fn bounded_value(value: Option<&str>, default: u64, minimum: u64, maximum: u64) -> u64 {
    value
        .and_then(|value| value.parse().ok())
        .map_or(default, |value: u64| value.clamp(minimum, maximum))
}

thread_local! {
    static THREAD_COLLECTOR: RefCell<ThreadCollector> = RefCell::new(ThreadCollector::from_process_config());
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

/// Drains the calling thread's profile into the bounded process collector.
///
/// Disabled profiles are discarded. Enabled profiles, including profiles that
/// contain a PMU failure, remain available as workload evidence.
pub fn submit_thread_profile(label: &'static str) {
    let profile = take_thread_profile();
    process_collector().submit(label, profile);
}

/// Removes all profiles submitted by worker threads since the previous drain.
#[must_use]
pub fn take_process_profiles() -> ProcessProfileBatch {
    process_collector().take()
}

/// Clears submitted profiles and stale records on the calling control thread.
pub fn clear_process_profiles() {
    process_collector().clear();
    let _ = take_thread_profile();
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
            collector
                .borrow_mut()
                .finish(self.name, self.started.clone());
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
        Self::open_set(CounterSet::General)
    }

    pub fn open_set(counter_set: CounterSet) -> Result<Self, PmuFailure> {
        #[cfg(target_os = "linux")]
        {
            linux::LinuxCounterGroup::open(counter_set).map(|inner| Self { inner })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(PmuFailure::unsupported(
                "Linux perf_event_open is unavailable on this platform",
            ))
        }
    }

    pub fn open_with_diagnostics() -> (Result<Self, PmuFailure>, PmuOpenDiagnostics) {
        Self::open_set_with_diagnostics(CounterSet::General)
    }

    pub fn open_set_with_diagnostics(
        counter_set: CounterSet,
    ) -> (Result<Self, PmuFailure>, PmuOpenDiagnostics) {
        #[cfg(target_os = "linux")]
        {
            let (inner, diagnostics) =
                linux::LinuxCounterGroup::open_with_diagnostics(counter_set);
            (inner.map(|inner| Self { inner }), diagnostics)
        }
        #[cfg(not(target_os = "linux"))]
        {
            (
                Err(PmuFailure::unsupported(
                    "Linux perf_event_open is unavailable on this platform",
                )),
                PmuOpenDiagnostics::default(),
            )
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

    #[must_use]
    pub fn read_format(&self) -> GroupReadFormat {
        #[cfg(target_os = "linux")]
        {
            self.inner.read_format()
        }
        #[cfg(not(target_os = "linux"))]
        {
            GroupReadFormat::OrderedValues
        }
    }

    #[must_use]
    pub fn scope(&self) -> CounterScope {
        #[cfg(target_os = "linux")]
        {
            self.inner.scope()
        }
        #[cfg(not(target_os = "linux"))]
        {
            CounterScope::CallingThreadAnyCpu
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

#[cfg(any(target_os = "linux", test))]
fn decode_group_read(
    words: &[u64],
    counter_set: CounterSet,
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
        counter_set,
        time_enabled_ns: words[1],
        time_running_ns: words[2],
        counters,
    })
}

#[cfg(any(target_os = "linux", test))]
fn decode_ordered_group_read(
    words: &[u64],
    counter_set: CounterSet,
) -> Result<CounterSnapshot, PmuFailure> {
    let events = counter_set.events();
    if words.len() != events.len() + 1 || words[0] != events.len() as u64 {
        return Err(PmuFailure::malformed(format!(
            "ordered group read reported {} words and {} events, expected {} words and {} events",
            words.len(),
            words.first().copied().unwrap_or(0),
            events.len() + 1,
            events.len()
        )));
    }
    let mut counters = CounterValues::default();
    for (event, value) in events.iter().copied().zip(words[1..].iter().copied()) {
        counters.set(event, value);
    }
    Ok(CounterSnapshot {
        counter_set,
        time_enabled_ns: 0,
        time_running_ns: 0,
        counters,
    })
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{
        CounterScope, CounterSet, CounterSnapshot, GroupReadFormat, HardwareEvent, PmuEnvironment,
        PmuFailure, PmuOpenAttempt, PmuOpenDiagnostics, decode_group_read,
        decode_ordered_group_read,
    };
    use std::collections::BTreeMap;

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
        counter_set: CounterSet,
        read_format: GroupReadFormat,
        scope: CounterScope,
    }

    #[derive(Clone, Copy)]
    struct OpenOptions {
        read_format: GroupReadFormat,
        scope: CounterScope,
        cpu: libc::c_int,
        perf_flags: libc::c_ulong,
    }

    impl LinuxCounterGroup {
        pub(super) fn open(counter_set: CounterSet) -> Result<Self, PmuFailure> {
            Self::open_matrix(counter_set, None)
        }

        pub(super) fn open_with_diagnostics(
            counter_set: CounterSet,
        ) -> (Result<Self, PmuFailure>, PmuOpenDiagnostics) {
            let mut diagnostics = PmuOpenDiagnostics {
                environment: read_environment(),
                attempts: Vec::new(),
            };
            diagnostics.attempts.extend([
                cycle_attribute_attempt("cycle-minimal", 0, 0),
                cycle_attribute_attempt("cycle-disabled", 0, PERF_ATTR_DISABLED),
                cycle_attribute_attempt("cycle-exclude-kernel", 0, PERF_ATTR_EXCLUDE_KERNEL),
                cycle_attribute_attempt(
                    "cycle-disabled-exclude-kernel",
                    0,
                    PERF_ATTR_DISABLED | PERF_ATTR_EXCLUDE_KERNEL,
                ),
                cycle_attribute_attempt("cycle-group-format", PERF_FORMAT_GROUP, 0),
                cycle_attribute_attempt(
                    "cycle-group-format-disabled",
                    PERF_FORMAT_GROUP,
                    PERF_ATTR_DISABLED,
                ),
                cycle_attribute_attempt(
                    "cycle-group-format-disabled-exclude-kernel",
                    PERF_FORMAT_GROUP,
                    PERF_ATTR_DISABLED | PERF_ATTR_EXCLUDE_KERNEL,
                ),
                cycle_attribute_attempt(
                    "cycle-group-format-all-exclusions",
                    PERF_FORMAT_GROUP,
                    PERF_ATTR_DISABLED | PERF_ATTR_EXCLUDE_KERNEL | PERF_ATTR_EXCLUDE_HYPERVISOR,
                ),
            ]);
            let group = Self::open_matrix(counter_set, Some(&mut diagnostics.attempts));
            (group, diagnostics)
        }

        fn open_matrix(
            counter_set: CounterSet,
            mut attempts: Option<&mut Vec<PmuOpenAttempt>>,
        ) -> Result<Self, PmuFailure> {
            let options = [
                OpenOptions {
                    read_format: GroupReadFormat::IdsAndTimes,
                    scope: CounterScope::CallingThreadAnyCpu,
                    cpu: -1,
                    perf_flags: PERF_FLAG_FD_CLOEXEC,
                },
                OpenOptions {
                    read_format: GroupReadFormat::OrderedValues,
                    scope: CounterScope::CallingThreadAnyCpu,
                    cpu: -1,
                    perf_flags: PERF_FLAG_FD_CLOEXEC,
                },
                OpenOptions {
                    read_format: GroupReadFormat::OrderedValues,
                    scope: CounterScope::CallingThreadAnyCpu,
                    cpu: -1,
                    perf_flags: 0,
                },
            ];
            let mut last_failure = None;
            for options in options {
                match Self::open_with_options(counter_set, options) {
                    Ok(group) => {
                        if let Some(attempts) = attempts.as_deref_mut() {
                            attempts.push(group_attempt(options, None));
                        }
                        return Ok(group);
                    }
                    Err(failure) if compatibility_failure(&failure) => {
                        if let Some(attempts) = attempts.as_deref_mut() {
                            attempts.push(group_attempt(options, Some(failure.clone())));
                        }
                        last_failure = Some(failure);
                    }
                    Err(failure) => {
                        if let Some(attempts) = attempts.as_deref_mut() {
                            attempts.push(group_attempt(options, Some(failure.clone())));
                        }
                        return Err(failure);
                    }
                }
            }
            Err(last_failure.expect("PMU open matrix is nonempty"))
        }

        fn open_with_options(
            counter_set: CounterSet,
            options: OpenOptions,
        ) -> Result<Self, PmuFailure> {
            debug_assert_eq!(std::mem::size_of::<PerfEventAttr>(), 64);
            let leader = open_event(HardwareEvent::Cycles, -1, options)?;
            let mut group = Self {
                descriptors: vec![Descriptor { fd: leader }],
                event_ids: BTreeMap::new(),
                counter_set,
                read_format: options.read_format,
                scope: options.scope,
            };
            for event in counter_set.events().iter().copied().skip(1) {
                let descriptor = open_event(event, leader, options)?;
                group.descriptors.push(Descriptor { fd: descriptor });
            }
            if options.read_format == GroupReadFormat::IdsAndTimes {
                for (descriptor, event) in group
                    .descriptors
                    .iter()
                    .zip(counter_set.events().iter().copied())
                {
                    let id = event_id(descriptor.fd, event)?;
                    if group.event_ids.insert(id, event).is_some() {
                        return Err(PmuFailure::malformed(format!(
                            "duplicate kernel event id {id}"
                        )));
                    }
                }
            }
            group.ioctl_group(PERF_EVENT_IOC_RESET, "reset-group")?;
            group.ioctl_group(PERF_EVENT_IOC_ENABLE, "enable-group")?;
            Ok(group)
        }

        pub(super) fn snapshot(&self) -> Result<CounterSnapshot, PmuFailure> {
            match self.read_format {
                GroupReadFormat::IdsAndTimes => self.snapshot_with_ids_and_times(),
                GroupReadFormat::OrderedValues => self.snapshot_ordered_values(),
            }
        }

        pub(super) const fn read_format(&self) -> GroupReadFormat {
            self.read_format
        }

        pub(super) const fn scope(&self) -> CounterScope {
            self.scope
        }

        fn snapshot_with_ids_and_times(&self) -> Result<CounterSnapshot, PmuFailure> {
            let mut words = vec![0_u64; 3 + self.counter_set.events().len() * 2];
            let expected_bytes = std::mem::size_of_val(words.as_slice());
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
            decode_group_read(&words, self.counter_set, &self.event_ids)
        }

        fn snapshot_ordered_values(&self) -> Result<CounterSnapshot, PmuFailure> {
            let mut words = vec![0_u64; self.counter_set.events().len() + 1];
            let expected_bytes = std::mem::size_of_val(words.as_slice());
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
                    "ordered group read returned {read_bytes} bytes, expected {expected_bytes}"
                )));
            }
            decode_ordered_group_read(&words, self.counter_set)
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
                Err(PmuFailure::io(stage, None, std::io::Error::last_os_error()))
            }
        }
    }

    fn compatibility_failure(failure: &PmuFailure) -> bool {
        matches!(
            failure.errno,
            Some(libc::EINVAL | libc::ENOTTY | libc::EOPNOTSUPP)
        )
    }

    fn read_environment() -> PmuEnvironment {
        let mut event_sources = std::fs::read_dir("/sys/bus/event_source/devices")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        event_sources.sort();
        let perf_event_paranoid = std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
            .ok()
            .map(|value| value.trim().to_owned());
        PmuEnvironment {
            event_sources,
            perf_event_paranoid,
        }
    }

    fn cycle_attribute_attempt(name: &str, read_format: u64, flags: u64) -> PmuOpenAttempt {
        let attributes = PerfEventAttr {
            event_type: HardwareEvent::Cycles.perf_type(),
            size: std::mem::size_of::<PerfEventAttr>() as u32,
            config: HardwareEvent::Cycles.perf_config(),
            read_format,
            flags,
            ..PerfEventAttr::default()
        };
        let result = perf_event_open(
            &attributes,
            HardwareEvent::Cycles,
            -1,
            -1,
            PERF_FLAG_FD_CLOEXEC,
        )
        .and_then(|descriptor| {
            // SAFETY: the descriptor is uniquely owned by this diagnostic attempt.
            if unsafe { libc::close(descriptor) } == 0 {
                Ok(())
            } else {
                Err(PmuFailure::io(
                    "close-minimal-event",
                    Some(HardwareEvent::Cycles),
                    std::io::Error::last_os_error(),
                ))
            }
        });
        PmuOpenAttempt {
            name: name.to_owned(),
            read_format: if read_format == 0 {
                None
            } else {
                Some(GroupReadFormat::OrderedValues)
            },
            scope: CounterScope::CallingThreadAnyCpu,
            cpu: -1,
            perf_flags: PERF_FLAG_FD_CLOEXEC as u64,
            disabled: flags & PERF_ATTR_DISABLED != 0,
            exclude_kernel: flags & PERF_ATTR_EXCLUDE_KERNEL != 0,
            exclude_hypervisor: flags & PERF_ATTR_EXCLUDE_HYPERVISOR != 0,
            grouped: false,
            success: result.is_ok(),
            failure: result.err(),
        }
    }

    fn group_attempt(options: OpenOptions, failure: Option<PmuFailure>) -> PmuOpenAttempt {
        PmuOpenAttempt {
            name: match (
                options.read_format,
                options.perf_flags == PERF_FLAG_FD_CLOEXEC,
            ) {
                (GroupReadFormat::IdsAndTimes, true) => "group-ids-times",
                (GroupReadFormat::OrderedValues, true) => "group-ordered",
                (GroupReadFormat::OrderedValues, false) => "group-legacy-flags",
                _ => "group-other",
            }
            .to_owned(),
            read_format: Some(options.read_format),
            scope: options.scope,
            cpu: options.cpu,
            perf_flags: options.perf_flags as u64,
            disabled: true,
            exclude_kernel: false,
            exclude_hypervisor: false,
            grouped: true,
            success: failure.is_none(),
            failure,
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
        options: OpenOptions,
    ) -> Result<libc::c_int, PmuFailure> {
        let attributes = PerfEventAttr {
            event_type: event.perf_type(),
            size: 64,
            config: event.perf_config(),
            read_format: match options.read_format {
                GroupReadFormat::IdsAndTimes => {
                    PERF_FORMAT_TOTAL_TIME_ENABLED
                        | PERF_FORMAT_TOTAL_TIME_RUNNING
                        | PERF_FORMAT_ID
                        | PERF_FORMAT_GROUP
                }
                GroupReadFormat::OrderedValues => PERF_FORMAT_GROUP,
            },
            flags: PERF_ATTR_DISABLED,
            ..PerfEventAttr::default()
        };
        let descriptor = perf_event_open(
            &attributes,
            event,
            options.cpu,
            group_descriptor,
            options.perf_flags,
        )?;
        if options.perf_flags == 0 {
            // SAFETY: the descriptor was returned by perf_event_open and the
            // integer fcntl command does not dereference a pointer.
            if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                let error = std::io::Error::last_os_error();
                unsafe { libc::close(descriptor) };
                return Err(PmuFailure::io("set-close-on-exec", Some(event), error));
            }
        }
        Ok(descriptor)
    }

    fn perf_event_open(
        attributes: &PerfEventAttr,
        event: HardwareEvent,
        cpu: libc::c_int,
        group_descriptor: libc::c_int,
        perf_flags: libc::c_ulong,
    ) -> Result<libc::c_int, PmuFailure> {
        // SAFETY: the syscall receives a valid version-zero attribute structure
        // and requests counters for the calling thread on any CPU.
        let raw = unsafe {
            libc::syscall(
                libc::SYS_perf_event_open,
                attributes,
                0,
                cpu,
                group_descriptor,
                perf_flags,
            )
        };
        if raw < 0 {
            return Err(PmuFailure::io(
                "open-event",
                Some(event),
                std::io::Error::last_os_error(),
            ));
        }
        let descriptor = libc::c_int::try_from(raw)
            .map_err(|_| PmuFailure::malformed("perf event descriptor exceeds c_int"))?;
        Ok(descriptor)
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

    #[test]
    fn typed_profile_config_preserves_bounds_and_disabled_defaults() {
        let values = std::collections::BTreeMap::from([
            (PMU_PROFILE, "1"),
            (PMU_SAMPLE_EVERY, "0"),
            (PMU_RECORD_LIMIT, "999999"),
        ]);
        let config = PmuProfileConfig::capture_with(|name| values.get(name).copied());

        assert!(config.enabled);
        assert_eq!(config.sample_every, 1);
        assert_eq!(config.record_limit, 65_536);
        assert!(!PmuProfileConfig::capture_with(|_| None).enabled);
    }
    use std::sync::Mutex;

    static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn ids() -> BTreeMap<u64, HardwareEvent> {
        GENERAL_EVENTS
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
        let snapshot = decode_group_read(&words, CounterSet::General, &ids()).unwrap();
        assert_eq!(snapshot.time_enabled_ns, 1_000);
        assert_eq!(snapshot.time_running_ns, 900);
        assert_eq!(snapshot.counters.get(HardwareEvent::Cycles), Some(60));
        assert_eq!(snapshot.counters.get(HardwareEvent::Instructions), Some(45));
        assert_eq!(snapshot.counters.get(HardwareEvent::L1dAccesses), Some(30));
        assert_eq!(snapshot.counters.get(HardwareEvent::L1dRefills), Some(4));
        assert_eq!(snapshot.counters.get(HardwareEvent::Branches), Some(20));
        assert_eq!(snapshot.counters.get(HardwareEvent::BranchMispredicts), Some(3));
    }

    #[test]
    fn malformed_group_reads_are_rejected() {
        assert!(decode_group_read(&[6, 1, 1], CounterSet::General, &ids()).is_err());
        let mut wrong_count = [0_u64; 15];
        wrong_count[0] = 5;
        assert!(decode_group_read(&wrong_count, CounterSet::General, &ids()).is_err());
        let unknown = [6, 1, 1, 1, 999, 1, 100, 1, 101, 1, 102, 1, 103, 1, 104];
        assert!(decode_group_read(&unknown, CounterSet::General, &ids()).is_err());
    }

    #[test]
    fn legacy_ordered_group_reads_preserve_declared_event_order() {
        let snapshot =
            decode_ordered_group_read(&[6, 60, 45, 30, 4, 20, 3], CounterSet::General).unwrap();
        assert_eq!(snapshot.counters.get(HardwareEvent::Cycles), Some(60));
        assert_eq!(snapshot.counters.get(HardwareEvent::Instructions), Some(45));
        assert_eq!(snapshot.counters.get(HardwareEvent::L1dAccesses), Some(30));
        assert_eq!(snapshot.counters.get(HardwareEvent::L1dRefills), Some(4));
        assert_eq!(snapshot.counters.get(HardwareEvent::Branches), Some(20));
        assert_eq!(snapshot.counters.get(HardwareEvent::BranchMispredicts), Some(3));
        assert!(decode_ordered_group_read(&[5, 1, 2, 3, 4, 5], CounterSet::General).is_err());
    }

    #[test]
    fn counter_deltas_saturate_and_ratios_are_bounded_by_zero_denominators() {
        let earlier = CounterSnapshot {
            counter_set: CounterSet::General,
            time_enabled_ns: 100,
            time_running_ns: 80,
            counters: CounterValues::from([
                (HardwareEvent::Cycles, 100),
                (HardwareEvent::Instructions, 60),
                (HardwareEvent::L1dAccesses, 20),
                (HardwareEvent::L1dRefills, 5),
                (HardwareEvent::Branches, 10),
                (HardwareEvent::BranchMispredicts, 2),
            ]),
        };
        let later = CounterSnapshot {
            counter_set: CounterSet::General,
            time_enabled_ns: 90,
            time_running_ns: 90,
            counters: CounterValues::from([
                (HardwareEvent::Cycles, 160),
                (HardwareEvent::Instructions, 105),
                (HardwareEvent::L1dAccesses, 30),
                (HardwareEvent::L1dRefills, 7),
                (HardwareEvent::Branches, 15),
                (HardwareEvent::BranchMispredicts, 3),
            ]),
        };
        let delta = later.delta_from(earlier);
        assert_eq!(delta.time_enabled_ns, 0);
        assert_eq!(delta.time_running_ns, 10);
        assert_eq!(delta.counters.get(HardwareEvent::Cycles), Some(60));
        assert_eq!(delta.instructions_per_cycle(), 0.75);
        assert_eq!(delta.l1d_refill_percent(), 20.0);
        assert_eq!(CounterDelta::default().cycles_per_instruction(), 0.0);
    }

    #[test]
    fn overlapping_spans_form_independent_nested_deltas() {
        let outer_start = CounterSnapshot::default();
        let inner_start = CounterSnapshot {
            counter_set: CounterSet::General,
            time_enabled_ns: 10,
            time_running_ns: 8,
            counters: CounterValues::from([
                (HardwareEvent::Cycles, 100),
                (HardwareEvent::Instructions, 70),
            ]),
        };
        let inner_end = CounterSnapshot {
            counter_set: CounterSet::General,
            time_enabled_ns: 20,
            time_running_ns: 16,
            counters: CounterValues::from([
                (HardwareEvent::Cycles, 180),
                (HardwareEvent::Instructions, 130),
            ]),
        };
        let outer_end = CounterSnapshot {
            counter_set: CounterSet::General,
            time_enabled_ns: 30,
            time_running_ns: 24,
            counters: CounterValues::from([
                (HardwareEvent::Cycles, 240),
                (HardwareEvent::Instructions, 170),
            ]),
        };
        assert_eq!(
            inner_end
                .delta_from(inner_start)
                .counters
                .get(HardwareEvent::Cycles),
            Some(80)
        );
        assert_eq!(
            outer_end
                .delta_from(outer_start)
                .counters
                .get(HardwareEvent::Cycles),
            None
        );
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
        assert!(value["counters"]["counters"].as_object().unwrap().is_empty());
    }

    #[test]
    fn event_metadata_declares_the_complete_non_multiplexed_set() {
        let metadata = event_metadata();
        assert_eq!(metadata.len(), 6);
        assert_eq!(metadata[2].semantic, "Cortex-A9 L1 data-cache accesses");
        assert_eq!(metadata[3].semantic, "Cortex-A9 L1 data-cache refills");
        let neon = event_metadata_for(CounterSet::CortexA9Neon);
        assert_eq!(neon.len(), 7);
        assert_eq!(neon[2].event, HardwareEvent::NeonInstructions);
        assert_eq!(neon[2].perf_type, 4);
        assert_eq!(neon[2].perf_config, 0x74);
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

    fn test_profile(enabled: bool) -> ThreadProfile {
        ThreadProfile {
            schema: "mister-magik-pmu-thread-profile-v1",
            enabled,
            sample_every: 1,
            attempted_spans: 0,
            dropped_spans: 0,
            records: Vec::new(),
            failure: None,
            read_format: None,
            scope: None,
        }
    }

    #[test]
    fn process_collector_discards_disabled_profiles_and_drains() {
        let mut collector = ProcessCollector::default();
        collector.submit("disabled", test_profile(false));
        collector.submit("builder", test_profile(true));

        let batch = collector.take();
        assert_eq!(batch.profiles.len(), 1);
        assert_eq!(batch.profiles[0].label, "builder");
        assert_eq!(batch.dropped_profiles, 0);
        assert_eq!(collector.take(), ProcessProfileBatch::default());
    }

    #[test]
    fn process_collector_bounds_profiles_and_reports_overflow() {
        let mut collector = ProcessCollector::default();
        for _ in 0..PROCESS_PROFILE_LIMIT + 3 {
            collector.submit("worker", test_profile(true));
        }

        let batch = collector.take();
        assert_eq!(batch.profiles.len(), PROCESS_PROFILE_LIMIT);
        assert_eq!(batch.dropped_profiles, 3);
    }

    #[test]
    fn process_collector_preserves_failures_as_evidence() {
        let mut profile = test_profile(true);
        profile.failure = Some(PmuFailure::unsupported("counter unavailable"));
        let mut collector = ProcessCollector::default();
        collector.submit("worker", profile);

        let batch = collector.take();
        assert_eq!(batch.profiles.len(), 1);
        assert_eq!(
            batch.profiles[0]
                .profile
                .failure
                .as_ref()
                .map(|failure| failure.message.as_str()),
            Some("counter unavailable")
        );
    }

    #[test]
    fn public_process_collection_accepts_multiple_threads_and_resets() {
        let _guard = PROCESS_TEST_LOCK.lock().unwrap();
        clear_process_profiles();
        let workers = ["walker", "publisher"].map(|label| {
            std::thread::spawn(move || {
                THREAD_COLLECTOR.with(|collector| {
                    *collector.borrow_mut() = ThreadCollector::new(true, 1, 1);
                });
                submit_thread_profile(label);
            })
        });
        for worker in workers {
            worker.join().unwrap();
        }

        let mut labels = take_process_profiles()
            .profiles
            .into_iter()
            .map(|entry| entry.label)
            .collect::<Vec<_>>();
        labels.sort();
        assert_eq!(labels, ["publisher", "walker"]);
        clear_process_profiles();
        assert_eq!(take_process_profiles(), ProcessProfileBatch::default());
    }
}
