//! Opt-in application support shared by Mini-MagiK and MiSTer MagiK.
//! The application supplies its own pixels and confirmed presentation counters.
pub mod frame_evidence;
pub mod measurement;
mod preview;
mod profile;
pub mod scheduling;
use measurement::PresentationMetrics;
use preview::PreviewProducer;
pub use profile::CpuProfile;
use slint::platform::software_renderer::Rgb565Pixel;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// Measurement windows start after this device-clock warmup.
const MEASUREMENT_WARMUP_MS: u64 = 2_000;
/// Live metrics refresh while the UI is idle, and at most this often in motion.
const PERIODIC_WRITE_INTERVAL: Duration = Duration::from_millis(200);
const MOTION_WRITE_INTERVAL: Duration = Duration::from_secs(5);
/// The scheduling baseline is taken this long before the window starts.
const SCHEDULING_SNAPSHOT_LEAD_MS: u64 = 500;

/// Right taps, then a right hold, timed from the measurement window start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CarouselSequence {
    taps: u32,
    tap_interval_ms: u64,
    hold_ms: u64,
}

impl CarouselSequence {
    fn from_request(value: &serde_json::Value) -> Option<Self> {
        let taps = value["taps"]
            .as_u64()
            .filter(|taps| (1..=20).contains(taps))?;
        Some(Self {
            taps: taps as u32,
            tap_interval_ms: value["tap_interval_ms"]
                .as_u64()
                .filter(|ms| (100..=2_000).contains(ms))?,
            hold_ms: value["hold_ms"]
                .as_u64()
                .filter(|ms| (1_000..=20_000).contains(ms))?,
        })
    }

    fn hold_window_ms(self) -> (u64, u64) {
        let start = u64::from(self.taps) * self.tap_interval_ms;
        (start, start + self.hold_ms)
    }
}

pub struct Session {
    pub metrics: PresentationMetrics,
    start: Instant,
    root: PathBuf,
    previews: PreviewProducer,
    profile: Option<CpuProfile>,
    last_write: Instant,
    last_request: Instant,
    ready: bool,
    clock_mode: Option<bool>,
    measurement_duration_ms: Option<u64>,
    clock_advanced: bool,
    force_card_fallback: bool,
    carousel_hold_requested: bool,
    carousel_hold_active: bool,
    /// The held direction: down (a list scrolling) instead of right (the carousel).
    carousel_hold_down: bool,
    carousel_sequence: Option<CarouselSequence>,
    carousel_taps_sent: u32,
    /// Whether windows record system scheduling evidence. Each snapshot scans
    /// procfs for tens of milliseconds of real time.
    scheduling_evidence: bool,
    /// Device ms and system snapshot taken late in the warmup.
    scheduling_start: Option<(u64, scheduling::Snapshot)>,
    /// The application's UI is in motion; periodic writes yield to it.
    ui_motion: bool,
    /// The latest request asked for the screensaver during its window.
    screensaver_requested: bool,
}
impl Session {
    pub fn from_environment() -> Option<Self> {
        let root = PathBuf::from(std::env::var_os("MISTER_MAGIK2_STATE_ROOT")?);
        Some(Self::new(root))
    }

    /// A session owns its artifact directory; explicit construction also lets
    /// launcher sequence tests exercise it without changing process environment.
    pub fn new(root: PathBuf) -> Self {
        Self {
            metrics: PresentationMetrics::default(),
            start: Instant::now(),
            root,
            previews: PreviewProducer::new(),
            profile: None,
            last_write: Instant::now() - Duration::from_secs(1),
            last_request: Instant::now(),
            ready: false,
            clock_mode: None,
            measurement_duration_ms: None,
            clock_advanced: false,
            force_card_fallback: false,
            carousel_hold_requested: false,
            carousel_hold_active: false,
            carousel_hold_down: false,
            carousel_sequence: None,
            carousel_taps_sent: 0,
            scheduling_evidence: true,
            scheduling_start: None,
            ui_motion: false,
            screensaver_requested: false,
        }
    }
    /// Report whether the application's UI is in motion. Periodic metric
    /// writes are postponed while it is, for at most five seconds; completed
    /// windows and readiness are always written immediately.
    pub fn set_ui_motion(&mut self, active: bool) {
        self.ui_motion = active;
    }

    pub fn set_measurement_duration(&mut self, milliseconds: Option<u64>) {
        self.measurement_duration_ms = milliseconds;
    }
    /// Prepare at loop entry, including warmup: tick may open the window later
    /// in this iteration. Only an open window retains the finished record.
    pub fn frame_evidence_candidate(
        &self,
        attempt_id: u64,
        begin_us: u64,
    ) -> Option<frame_evidence::FrameEvidence> {
        (self.metrics.frame_evidence.mode != frame_evidence::EvidenceMode::Off
            && self.metrics.motion_started_ms.is_some()
            && self.metrics.window.is_none())
        .then(|| frame_evidence::FrameEvidence {
            phases_enabled: self.metrics.frame_evidence.mode
                == frame_evidence::EvidenceMode::Phases,
            attempt_id,
            begin_us,
            ..Default::default()
        })
    }

    pub fn record_frame_evidence(
        &mut self,
        frame: frame_evidence::FrameEvidence,
        started: Instant,
    ) {
        if self.frame_evidence_active() {
            self.metrics.frame_evidence.observe_timed(frame, started);
        }
    }

    pub fn frame_evidence_active(&self) -> bool {
        self.metrics.frame_evidence.mode != frame_evidence::EvidenceMode::Off
            && self.metrics.window_start.is_some()
            && self.metrics.window.is_none()
    }

    pub fn begin(&mut self) {
        self.metrics.dropped_frame_records.clear();
        self.metrics.last_dropped_frame = None;
        self.metrics.dropped_frame_records_omitted = 0;
        self.metrics.dropped_frames_by_workload = [0; 5];
        self.metrics.moving_cpu_us = None;
        self.metrics.moving_presentations = 0;
        self.metrics.moving_cpu_unavailable_intervals = 0;
        self.metrics.previous_motion_cpu_sample = None;
        self.metrics.dropped_frame_records.reserve(64);
        self.metrics.work_timings.clear();
        self.metrics.work_timings.reserve(3601);
        self.metrics.frame_timings_us.clear();
        self.metrics.frame_timings_us.reserve(3601);
        self.metrics.motion_started_ms = Some(self.start.elapsed().as_millis() as u64);
        // A restarted measurement takes its own scheduling baseline.
        self.scheduling_start = None;
        self.metrics.window_start = None;
        self.metrics.window = None;
        self.clock_advanced = false;
        self.metrics.forced_clock_changes = 0;
    }
    /// Test-only display clock; elapsed device time triggers exactly one update.
    /// The host and OS wall clocks never participate in this workload.
    pub fn launcher_clock(&mut self) -> Option<&'static str> {
        let rollover = self.clock_mode?;
        if rollover
            && !self.clock_advanced
            && self
                .metrics
                .window_start
                .as_ref()
                .is_some_and(|(start, _)| self.start.elapsed().as_millis() as u64 >= start + 2_500)
        {
            self.clock_advanced = true;
            self.metrics.forced_clock_changes += 1;
        }
        Some(if self.clock_advanced {
            "12:35"
        } else {
            "12:34"
        })
    }

    /// The screensaver should start immediately and stay up until the
    /// requested window completes.
    pub fn screensaver_requested(&self) -> bool {
        self.screensaver_requested
            && self.metrics.motion_started_ms.is_some()
            && self.metrics.window.is_none()
    }

    pub fn card_fallback_forced(&self) -> bool {
        self.force_card_fallback
    }
    /// Whether the requested hold presses Down (scrolling a list), not Right.
    pub fn carousel_hold_down(&self) -> bool {
        self.carousel_hold_down
    }

    /// Emit one logical press, then release on completion or explicit cancellation.
    /// A failed host cannot extend the hold beyond this bounded device window.
    pub fn carousel_hold_change(&mut self) -> Option<bool> {
        let sequence_holding = self.carousel_sequence.is_some_and(|sequence| {
            let (start, end) = sequence.hold_window_ms();
            self.window_elapsed_ms()
                .is_some_and(|elapsed| (start..end).contains(&elapsed))
        });
        let requested = (self.carousel_hold_requested || sequence_holding)
            && self.metrics.motion_started_ms.is_some()
            && self.metrics.window.is_none();
        if requested == self.carousel_hold_active {
            return None;
        }
        self.carousel_hold_active = requested;
        Some(requested)
    }

    /// One logical tap of a requested sequence is due. Taps start with the
    /// measurement window and never run after it completes.
    pub fn carousel_tap_due(&mut self) -> bool {
        let Some(sequence) = self.carousel_sequence else {
            return false;
        };
        let due = self.metrics.window.is_none()
            && self.carousel_taps_sent < sequence.taps
            && self.window_elapsed_ms().is_some_and(|elapsed| {
                elapsed >= u64::from(self.carousel_taps_sent) * sequence.tap_interval_ms
            });
        self.carousel_taps_sent += u32::from(due);
        due
    }

    /// A requested sequence's window is still open. The launcher must keep
    /// iterating until it completes: an idle loop sleeps until input arrives,
    /// so it would otherwise stall the taps or the settle after release.
    pub fn carousel_sequence_pending(&self) -> bool {
        self.carousel_sequence.is_some() && self.metrics.window.is_none()
    }

    /// Device time since the measurement window starts, once the warmup ends.
    fn window_elapsed_ms(&self) -> Option<u64> {
        let window_start = self.metrics.motion_started_ms? + MEASUREMENT_WARMUP_MS;
        (self.start.elapsed().as_millis() as u64).checked_sub(window_start)
    }

    fn measurement_duration(&self, instrumented: bool) -> u64 {
        // Preserve the legacy ten-second profile minimum; explicit longer
        // windows must cover the whole requested diagnostic workload.
        self.measurement_duration_ms
            .unwrap_or(5_000)
            .max(if instrumented { 10_000 } else { 0 })
    }

    /// Device-clock warmup and measurement boundaries, independent of host polling.
    pub fn tick(&mut self, width: usize, height: usize) -> Result<bool, String> {
        if self.last_request.elapsed() >= Duration::from_millis(100) {
            self.last_request = Instant::now();
            let request = self.root.join("measure-request");
            if request.exists() {
                let value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&request).map_err(|e| e.to_string())?)
                        .unwrap_or_default();
                if value["launcher_hold"] == "release" {
                    std::fs::remove_file(&request).map_err(|e| e.to_string())?;
                    self.carousel_hold_requested = false;
                    self.carousel_hold_down = false;
                    self.carousel_sequence = None;
                    self.screensaver_requested = false;
                } else {
                    let requested_duration_ms = if value["duration_ms"].is_null() {
                        None
                    } else {
                        Some(
                            value["duration_ms"]
                                .as_u64()
                                .filter(|duration| (1_000..=45_000).contains(duration))
                                .ok_or("duration_ms must be an integer between 1000 and 45000")?,
                        )
                    };
                    let evidence_mode =
                        frame_evidence::EvidenceMode::from_request(&value["frame_evidence"])?;
                    std::fs::remove_file(&request).map_err(|e| e.to_string())?;
                    self.screensaver_requested =
                        value["launcher_screensaver"].as_bool().unwrap_or(false);
                    self.carousel_hold_requested =
                        value["launcher_hold"].as_bool().unwrap_or(false);
                    self.carousel_hold_down = value["launcher_hold_direction"] == "down";
                    self.carousel_sequence =
                        CarouselSequence::from_request(&value["launcher_sequence"]);
                    self.carousel_taps_sent = 0;
                    self.clock_mode = match value["launcher_clock"].as_str() {
                        Some("fixed") => Some(false),
                        Some("rollover") => Some(true),
                        _ => None,
                    };
                    self.force_card_fallback =
                        value["launcher_fallback"].as_bool().unwrap_or(false);
                    self.measurement_duration_ms = requested_duration_ms;
                    self.metrics.frame_evidence.reset(evidence_mode);
                    self.begin();
                }
            }
        }
        let now = self.start.elapsed().as_millis() as u64;
        let instrumented = std::env::var_os("MISTER_MAGIK2_PROFILE_DIR").is_some();
        let duration = self.measurement_duration(instrumented);
        let mut completed = false;
        // Scanning procfs blocks for tens of milliseconds and a helper thread
        // would compete with rendering, so snapshot inside the warmup instead.
        if self.scheduling_evidence
            && self.metrics.window.is_none()
            && self.metrics.window_start.is_none()
            && self.scheduling_start.is_none()
            && self.metrics.motion_started_ms.is_some_and(|start| {
                now - start >= MEASUREMENT_WARMUP_MS - SCHEDULING_SNAPSHOT_LEAD_MS
            })
        {
            self.scheduling_start = Some((now, scheduling::snapshot()));
        }
        if self.metrics.window.is_none() {
            if self.metrics.window_start.is_none()
                && self
                    .metrics
                    .motion_started_ms
                    .is_some_and(|start| now - start >= MEASUREMENT_WARMUP_MS)
            {
                self.metrics.window_start = Some((now, self.metrics.counters.clone()));
                self.metrics.card_prepare_max_us = 0;
                self.metrics.window_cpu_start_us = self.metrics.process_cpu_us;
                self.profile = CpuProfile::start()?;
            }
            self.metrics.note_motion_cpu(self.ui_motion);
            if self
                .metrics
                .window_start
                .as_ref()
                .is_some_and(|(start, _)| now - start >= duration)
            {
                self.metrics.finish_window(now, width, height, instrumented);
                if self.metrics.frame_evidence.mode != frame_evidence::EvidenceMode::Off {
                    let evidence = self.metrics.frame_evidence.json();
                    if let Some(window) = self.metrics.window.as_mut() {
                        window["frame_evidence"] = evidence;
                    }
                }
                if let Some(window) = self.metrics.window.as_mut() {
                    window["requested_duration_ms"] =
                        serde_json::json!(self.measurement_duration_ms);
                    window["target_duration_ms"] = serde_json::json!(duration);
                }
                if let Some((taken_ms, start)) = self.scheduling_start.take()
                    && let Some(window) = self.metrics.window.as_mut()
                {
                    // Linux reports /proc/stat in USER_HZ, which is 100 on ARM.
                    let mut report = scheduling::report(&start, &scheduling::snapshot(), 100);
                    let window_start = window["start_ms"].as_u64().unwrap_or(taken_ms);
                    report["warmup_ms_included"] =
                        serde_json::json!(window_start.saturating_sub(taken_ms));
                    window["scheduling"] = report;
                }
                if let Some(profile) = self.profile.take() {
                    profile.finish()?;
                }
                completed = true;
            }
        }
        if !self.ready && self.metrics.counters.presentations > 0 {
            self.write("probe-ready.json", &self.metrics_json(width, height, now))?;
            self.ready = true;
        }
        let since_write = self.last_write.elapsed();
        let periodic_write_due = since_write >= PERIODIC_WRITE_INTERVAL
            && (!self.ui_motion || since_write >= MOTION_WRITE_INTERVAL);
        if completed || periodic_write_due {
            self.write("probe-metrics.json", &self.metrics_json(width, height, now))?;
            self.last_write = Instant::now();
        }
        Ok(completed)
    }
    /// Republish completed evidence after the app attaches its renderer report.
    pub fn publish_metrics(&mut self, width: usize, height: usize) -> Result<(), String> {
        let now = self.start.elapsed().as_millis() as u64;
        self.write("probe-metrics.json", &self.metrics_json(width, height, now))
    }

    fn metrics_json(&self, width: usize, height: usize, now: u64) -> serde_json::Value {
        let mut value = self.metrics.json(width, height, now);
        value["ui_motion"] = serde_json::json!(self.ui_motion);
        value
    }

    pub fn preview(&mut self, pixels: &[Rgb565Pixel], width: usize, height: usize) {
        self.previews
            .publish_if_watched(pixels, width, height, self.start.elapsed());
    }
    pub fn preview_rows(
        &mut self,
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
        stride: usize,
    ) {
        self.previews
            .publish_rows(pixels, width, height, stride, self.start.elapsed());
    }
    fn write(&self, name: &str, value: &serde_json::Value) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let temporary = self.root.join(format!("{name}.next"));
        std::fs::write(&temporary, value.to_string()).map_err(|e| e.to_string())?;
        std::fs::rename(temporary, self.root.join(name)).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_opening_iteration_keeps_its_first_frame_and_drop() {
        use frame_evidence::EvidenceMode;
        let root =
            std::env::temp_dir().join(format!("magik-first-evidence-{}", std::process::id()));
        let mut session = test_session(root.clone());
        session.metrics.frame_evidence.reset(EvidenceMode::Phases);
        session.set_measurement_duration(Some(1000));
        session.begin();
        // Loop entry occurs before tick opens the window. CPU phase zero can
        // already be filled on this candidate, just as in the launcher.
        let mut first = session.frame_evidence_candidate(1, 10).unwrap();
        assert!(!session.frame_evidence_active());
        first.cpu_us[0] = Some(12);
        session.start -= Duration::from_millis(MEASUREMENT_WARMUP_MS);
        session.tick(16, 8).unwrap();
        assert!(session.frame_evidence_active());
        first.record.dropped_frames = 1;
        session.metrics.counters.drops += 1;
        session.record_frame_evidence(first, Instant::now());
        session.start -= Duration::from_millis(1000);
        session.tick(16, 8).unwrap();
        let window = session.metrics.window.as_ref().unwrap();
        let frames = window["frame_evidence"]["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["attempt_id"], 1);
        assert_eq!(frames[0]["phases"]["cpu_us"][0], 12);
        assert_eq!(
            frames[0]["previous_read_bracket_us"],
            serde_json::Value::Null
        );
        assert_eq!(
            frames[0]["observation"]["dropped_frames"],
            window["dropped_frames"]
        );
        assert!(session.frame_evidence_candidate(2, 20).is_none());
        // A candidate created before a closing tick must also be excluded.
        session.record_frame_evidence(first, Instant::now());
        assert_eq!(session.metrics.frame_evidence.json()["observed_frames"], 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_evidence_request_preserves_session_and_can_be_corrected() {
        let root =
            std::env::temp_dir().join(format!("magik-invalid-evidence-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session = test_session(root.clone());
        session.carousel_hold_requested = true;
        session.measurement_duration_ms = Some(8000);
        session.carousel_sequence = Some(CarouselSequence {
            taps: 2,
            tap_interval_ms: 200,
            hold_ms: 1000,
        });
        session.carousel_taps_sent = 1;
        session.begin();
        let motion_started = session.metrics.motion_started_ms;
        let request = root.join("measure-request");
        std::fs::write(
            &request,
            r#"{"duration_ms":1000,"launcher_hold":false,"frame_evidence":"phase"}"#,
        )
        .unwrap();
        session.last_request -= Duration::from_millis(101);
        assert!(session.tick(16, 8).is_err());
        assert!(request.exists());
        assert!(session.carousel_hold_requested);
        assert_eq!(session.measurement_duration_ms, Some(8000));
        assert_eq!(session.carousel_taps_sent, 1);
        assert_eq!(session.carousel_sequence.unwrap().taps, 2);
        assert_eq!(session.metrics.motion_started_ms, motion_started);
        assert_eq!(
            session.metrics.frame_evidence.mode,
            frame_evidence::EvidenceMode::Off
        );
        std::fs::write(
            &request,
            r#"{"duration_ms":1000,"frame_evidence":"phases"}"#,
        )
        .unwrap();
        session.last_request -= Duration::from_millis(101);
        session.tick(16, 8).unwrap();
        assert!(!request.exists());
        assert_eq!(session.measurement_duration_ms, Some(1000));
        assert_eq!(
            session.metrics.frame_evidence.mode,
            frame_evidence::EvidenceMode::Phases
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A session with idle defaults; tests override only what they exercise.
    fn test_session(root: PathBuf) -> Session {
        let mut session = Session::new(root);
        session.last_write = Instant::now();
        session.scheduling_evidence = false;
        session
    }

    #[test]
    fn readiness_requires_a_presentation_and_measurements_exclude_warmup() {
        let root = std::env::temp_dir().join(format!("magik-session-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session = test_session(root.clone());
        session.tick(16, 8).unwrap();
        assert!(!root.join("probe-ready.json").exists());
        session.metrics.counters.presentations = 10;
        session.begin();
        session.start -= Duration::from_secs(3);
        session.tick(16, 8).unwrap();
        assert!(root.join("probe-ready.json").exists());
        session.metrics.counters.presentations = 30;
        session.start -= Duration::from_secs(5);
        assert!(session.tick(16, 8).unwrap());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("probe-metrics.json")).unwrap())
                .unwrap();
        assert_eq!(saved["window"]["presentations"], 20);
        assert_eq!(saved["window"]["instrumented"], false);
        session.clock_mode = Some(true);
        session.begin();
        assert_eq!(session.launcher_clock(), Some("12:34"));
        session.metrics.window_start = Some((
            session.start.elapsed().as_millis() as u64,
            session.metrics.counters.clone(),
        ));
        session.start -= Duration::from_millis(2501);
        assert_eq!(session.launcher_clock(), Some("12:35"));
        assert_eq!(session.launcher_clock(), Some("12:35"));
        assert_eq!(session.metrics.forced_clock_changes, 1);
        session.begin();
        assert_eq!(session.launcher_clock(), Some("12:34"));
        assert_eq!(session.metrics.forced_clock_changes, 0);
        session.clock_mode = Some(false);
        session.metrics.window_start = Some((0, session.metrics.counters.clone()));
        assert_eq!(session.launcher_clock(), Some("12:34"));
        assert_eq!(session.metrics.forced_clock_changes, 0);
        std::fs::write(
            root.join("measure-request"),
            r#"{"launcher_hold":true,"duration_ms":8000}"#,
        )
        .unwrap();
        session.last_request -= Duration::from_millis(101);
        session.tick(16, 8).unwrap();
        assert!(!session.carousel_hold_down());
        assert_eq!(session.measurement_duration_ms, Some(8000));
        assert_eq!(session.measurement_duration(true), 10_000);
        assert_eq!(session.carousel_hold_change(), Some(true));
        assert_eq!(session.carousel_hold_change(), None);
        session.start -= Duration::from_secs(3);
        session.tick(16, 8).unwrap();
        session.start -= Duration::from_secs(5);
        assert!(!session.tick(16, 8).unwrap());
        assert_eq!(session.carousel_hold_change(), None);
        // Cross the deadline deliberately: tick records actual elapsed time,
        // which also includes time spent running this test on the real clock.
        session.start -= Duration::from_millis(3001);
        assert!(session.tick(16, 8).unwrap());
        assert_eq!(session.carousel_hold_change(), Some(false));
        let window = session.metrics.window.as_ref().unwrap();
        assert_eq!(window["target_duration_ms"], 8000);
        assert!(window["elapsed_ms"].as_u64().unwrap() >= 8001);

        session.carousel_hold_requested = true;
        session.begin();
        assert_eq!(session.carousel_hold_change(), Some(true));
        let started = session.metrics.motion_started_ms;
        std::fs::write(
            root.join("measure-request"),
            r#"{"launcher_hold":"release"}"#,
        )
        .unwrap();
        session.last_request -= Duration::from_millis(101);
        session.tick(16, 8).unwrap();
        assert_eq!(session.carousel_hold_change(), Some(false));
        assert_eq!(session.metrics.motion_started_ms, started);
        // A list scroll holds Down, and a release clears the direction.
        std::fs::write(
            root.join("measure-request"),
            r#"{"launcher_hold":true,"launcher_hold_direction":"down","duration_ms":8000}"#,
        )
        .unwrap();
        session.last_request -= Duration::from_millis(101);
        session.tick(16, 8).unwrap();
        assert!(session.carousel_hold_down());
        std::fs::write(
            root.join("measure-request"),
            r#"{"launcher_hold":"release"}"#,
        )
        .unwrap();
        session.last_request -= Duration::from_millis(101);
        session.tick(16, 8).unwrap();
        assert!(!session.carousel_hold_down());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn long_requested_window_is_honoured_and_invalid_durations_do_not_start_measurement() {
        let root = std::env::temp_dir().join(format!("magik-duration-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session = test_session(root.clone());
        for invalid in [
            serde_json::json!(45001),
            serde_json::json!(-1),
            serde_json::json!("45000"),
        ] {
            std::fs::write(
                root.join("measure-request"),
                serde_json::json!({"duration_ms":invalid}).to_string(),
            )
            .unwrap();
            session.last_request -= Duration::from_millis(101);
            assert!(session.tick(16, 8).unwrap_err().contains("duration_ms"));
            assert!(session.metrics.motion_started_ms.is_none());
            assert!(root.join("measure-request").exists());
        }
        std::fs::write(root.join("measure-request"), r#"{"duration_ms":45000}"#).unwrap();
        session.last_request -= Duration::from_millis(101);
        session.tick(16, 8).unwrap();
        assert_eq!(session.measurement_duration_ms, Some(45000));
        assert_eq!(session.measurement_duration(true), 45_000);
        session.start -= Duration::from_millis(MEASUREMENT_WARMUP_MS);
        session.tick(16, 8).unwrap();
        session.start -= Duration::from_millis(45_000);
        session.tick(16, 8).unwrap();
        let window = session.metrics.window.as_ref().unwrap();
        assert_eq!(window["requested_duration_ms"], 45000);
        assert_eq!(window["target_duration_ms"], 45000);
        assert!(window["elapsed_ms"].as_u64().unwrap() >= 45000);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restarting_a_measurement_takes_a_fresh_scheduling_baseline() {
        let root = std::env::temp_dir().join(format!("magik-restart-{}", std::process::id()));
        let mut session = Session {
            scheduling_evidence: true,
            ..test_session(root.clone())
        };
        session.begin();
        session.start -= Duration::from_millis(MEASUREMENT_WARMUP_MS);
        session.tick(16, 8).unwrap();
        let (first_baseline_ms, _) = session.scheduling_start.as_ref().unwrap();
        let first_baseline_ms = *first_baseline_ms;
        session
            .metrics
            .record_dropped_frame(measurement::DroppedFrameRecord {
                dropped_frames: 3,
                workload: measurement::FrameWorkload::SystemTransition,
                ..Default::default()
            });
        assert_eq!(session.metrics.dropped_frames_by_workload[1], 3);
        session.begin();
        assert_eq!(session.metrics.dropped_frames_by_workload, [0; 5]);
        assert!(session.metrics.last_dropped_frame.is_none());
        assert!(session.metrics.dropped_frame_records.is_empty());
        assert_eq!(session.metrics.dropped_frame_records_omitted, 0);
        assert!(session.scheduling_start.is_none());
        session.start -= Duration::from_millis(MEASUREMENT_WARMUP_MS);
        session.tick(16, 8).unwrap();
        let (second_baseline_ms, _) = session.scheduling_start.as_ref().unwrap();
        assert!(*second_baseline_ms > first_baseline_ms);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn periodic_metric_writes_yield_to_ui_motion_but_completion_does_not() {
        let root = std::env::temp_dir().join(format!("magik-motion-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session {
            last_write: Instant::now() - Duration::from_millis(250),
            ready: true,
            ui_motion: true,
            ..test_session(root.clone())
        };
        let metrics = root.join("probe-metrics.json");
        session.tick(16, 8).unwrap();
        assert!(!metrics.exists(), "motion postpones a routine refresh");
        session.last_write -= MOTION_WRITE_INTERVAL;
        session.tick(16, 8).unwrap();
        assert!(metrics.exists(), "but only for a bounded time");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&metrics).unwrap()).unwrap();
        assert_eq!(value["ui_motion"], true);
        std::fs::remove_file(&metrics).unwrap();
        session.last_write = Instant::now() - Duration::from_millis(250);
        session.set_ui_motion(false);
        session.tick(16, 8).unwrap();
        assert!(metrics.exists(), "idle refreshes keep their cadence");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&metrics).unwrap()).unwrap();
        assert_eq!(value["ui_motion"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn carousel_sequence_taps_then_holds_inside_the_measurement_window() {
        let root = std::env::temp_dir().join(format!("magik-sequence-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session {
            start: Instant::now() - Duration::from_secs(60),
            last_request: Instant::now() - Duration::from_secs(1),
            ..test_session(root.clone())
        };
        std::fs::write(
            root.join("measure-request"),
            r#"{"duration_ms":12000,"launcher_sequence":
                {"taps":6,"tap_interval_ms":250,"hold_ms":10000}}"#,
        )
        .unwrap();
        session.tick(16, 8).unwrap();
        assert_eq!(session.measurement_duration_ms, Some(12_000));
        // Warmup: neither taps nor the hold start before the window, but the
        // launcher must keep iterating so an idle loop cannot stall them.
        assert!(session.carousel_sequence_pending());
        assert!(!session.carousel_tap_due());
        assert_eq!(session.carousel_hold_change(), None);
        let mut taps = 0;
        let mut hold_started_at = None;
        let mut hold_ended_at = None;
        for ms in (0..14_000).step_by(10) {
            session.metrics.motion_started_ms = Some(
                (session.start.elapsed().as_millis() as u64)
                    .saturating_sub(MEASUREMENT_WARMUP_MS + ms),
            );
            taps += u32::from(session.carousel_tap_due());
            match session.carousel_hold_change() {
                Some(true) => hold_started_at = Some(ms),
                Some(false) => hold_ended_at = Some(ms),
                None => {}
            }
            if ms < 1_500 {
                assert_eq!(taps, (ms / 250 + 1) as u32, "at {ms} ms");
            }
        }
        assert_eq!(taps, 6);
        assert!(
            session.carousel_sequence_pending(),
            "the settle after release stays awake until the window completes"
        );
        session.metrics.window = Some(serde_json::json!({}));
        assert!(!session.carousel_sequence_pending());
        session.metrics.window = None;
        assert_eq!(hold_started_at, Some(1_500));
        assert_eq!(hold_ended_at, Some(11_500));

        session.begin();
        session.carousel_sequence = None;
        std::fs::write(
            root.join("measure-request"),
            r#"{"launcher_sequence":{"taps":0,"tap_interval_ms":250,"hold_ms":10000}}"#,
        )
        .unwrap();
        session.last_request -= Duration::from_millis(101);
        session.tick(16, 8).unwrap();
        assert!(
            session.carousel_sequence.is_none(),
            "out-of-range sequences are ignored"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
