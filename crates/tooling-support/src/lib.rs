//! Opt-in application support shared by Mini-MagiK and MiSTer MagiK.
//! The application supplies its own pixels and confirmed presentation counters.
pub mod measurement;
mod preview;
mod profile;
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

/// Right taps, then a right hold, timed from the measurement window start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CarouselSequence {
    taps: u32,
    tap_interval_ms: u64,
    hold_ms: u64,
}

impl CarouselSequence {
    fn from_request(value: &serde_json::Value) -> Option<Self> {
        let taps = value["taps"].as_u64().filter(|taps| (1..=20).contains(taps))?;
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
    carousel_sequence: Option<CarouselSequence>,
    carousel_taps_sent: u32,
}
impl Session {
    pub fn from_environment() -> Option<Self> {
        let root = PathBuf::from(std::env::var_os("MISTER_MAGIK2_STATE_ROOT")?);
        Some(Self {
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
            carousel_sequence: None,
            carousel_taps_sent: 0,
        })
    }
    pub fn set_measurement_duration(&mut self, milliseconds: Option<u64>) {
        self.measurement_duration_ms = milliseconds;
    }
    pub fn begin(&mut self) {
        self.metrics.dropped_frame_records.clear();
        self.metrics.dropped_frame_records.reserve(64);
        self.metrics.work_timings.clear();
        self.metrics.work_timings.reserve(3601);
        self.metrics.frame_timings_us.clear();
        self.metrics.frame_timings_us.reserve(3601);
        self.metrics.motion_started_ms = Some(self.start.elapsed().as_millis() as u64);
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

    pub fn card_fallback_forced(&self) -> bool {
        self.force_card_fallback
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
                    self.carousel_hold_requested = false;
                    self.carousel_sequence = None;
                } else {
                    self.carousel_hold_requested =
                        value["launcher_hold"].as_bool().unwrap_or(false);
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
                    self.measurement_duration_ms = value["duration_ms"]
                        .as_u64()
                        .filter(|duration| (1_000..=30_000).contains(duration));
                    self.begin();
                }
                std::fs::remove_file(request).map_err(|e| e.to_string())?;
            }
        }
        let now = self.start.elapsed().as_millis() as u64;
        let instrumented = std::env::var_os("MISTER_MAGIK2_PROFILE_DIR").is_some();
        let duration = if instrumented {
            10_000
        } else {
            self.measurement_duration_ms.unwrap_or(5_000)
        };
        let mut completed = false;
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
            if self
                .metrics
                .window_start
                .as_ref()
                .is_some_and(|(start, _)| now - start >= duration)
            {
                self.metrics.finish_window(now, width, height, instrumented);
                if let Some(profile) = self.profile.take() {
                    profile.finish()?;
                }
                completed = true;
            }
        }
        if !self.ready && self.metrics.counters.presentations > 0 {
            self.write("probe-ready.json", &self.metrics.json(width, height, now))?;
            self.ready = true;
        }
        if completed || self.last_write.elapsed() >= Duration::from_millis(200) {
            self.write("probe-metrics.json", &self.metrics.json(width, height, now))?;
            self.last_write = Instant::now();
        }
        Ok(completed)
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
    fn readiness_requires_a_presentation_and_measurements_exclude_warmup() {
        let root = std::env::temp_dir().join(format!("magik-session-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session {
            metrics: PresentationMetrics::default(),
            start: Instant::now(),
            root: root.clone(),
            previews: PreviewProducer::new(),
            profile: None,
            last_write: Instant::now(),
            last_request: Instant::now(),
            ready: false,
            clock_mode: None,
            measurement_duration_ms: None,
            clock_advanced: false,
            force_card_fallback: false,
            carousel_hold_requested: false,
            carousel_hold_active: false,
            carousel_sequence: None,
            carousel_taps_sent: 0,
        };
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
        assert_eq!(session.measurement_duration_ms, Some(8000));
        assert_eq!(session.carousel_hold_change(), Some(true));
        assert_eq!(session.carousel_hold_change(), None);
        session.start -= Duration::from_secs(3);
        session.tick(16, 8).unwrap();
        session.start -= Duration::from_secs(5);
        assert!(!session.tick(16, 8).unwrap());
        assert_eq!(session.carousel_hold_change(), None);
        session.start -= Duration::from_secs(3);
        assert!(session.tick(16, 8).unwrap());
        assert_eq!(session.carousel_hold_change(), Some(false));
        assert_eq!(session.metrics.window.as_ref().unwrap()["elapsed_ms"], 8000);

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
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn carousel_sequence_taps_then_holds_inside_the_measurement_window() {
        let root = std::env::temp_dir().join(format!("magik-sequence-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Session {
            metrics: PresentationMetrics::default(),
            start: Instant::now() - Duration::from_secs(60),
            root: root.clone(),
            previews: PreviewProducer::new(),
            profile: None,
            last_write: Instant::now(),
            last_request: Instant::now() - Duration::from_secs(1),
            ready: false,
            clock_mode: None,
            measurement_duration_ms: None,
            clock_advanced: false,
            force_card_fallback: false,
            carousel_hold_requested: false,
            carousel_hold_active: false,
            carousel_sequence: None,
            carousel_taps_sent: 0,
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
                (session.start.elapsed().as_millis() as u64).saturating_sub(MEASUREMENT_WARMUP_MS + ms),
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
        assert!(session.carousel_sequence.is_none(), "out-of-range sequences are ignored");
        std::fs::remove_dir_all(root).unwrap();
    }
}
