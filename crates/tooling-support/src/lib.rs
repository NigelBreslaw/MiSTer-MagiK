//! Opt-in application support shared by Mini-MagiK and MiSTer MagiK.
//! The application supplies its own pixels and confirmed presentation counters.
pub mod measurement;
mod preview;
mod profile;
use measurement::PresentationMetrics;
use preview::PreviewProducer;
use profile::CpuProfile;
use slint::platform::software_renderer::Rgb565Pixel;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

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
        })
    }
    pub fn set_measurement_duration(&mut self, milliseconds: Option<u64>) {
        self.measurement_duration_ms = milliseconds;
    }
    pub fn begin(&mut self) {
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
    /// Device-clock warmup and measurement boundaries, independent of host polling.
    pub fn tick(&mut self, width: usize, height: usize) -> Result<bool, String> {
        if self.last_request.elapsed() >= Duration::from_millis(100) {
            self.last_request = Instant::now();
            let request = self.root.join("measure-request");
            if request.exists() {
                let value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&request).map_err(|e| e.to_string())?)
                        .unwrap_or_default();
                self.clock_mode = match value["launcher_clock"].as_str() {
                    Some("fixed") => Some(false),
                    Some("rollover") => Some(true),
                    _ => None,
                };
                self.force_card_fallback = value["launcher_fallback"].as_bool().unwrap_or(false);
                std::fs::remove_file(request).map_err(|e| e.to_string())?;
                self.begin();
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
                    .is_some_and(|start| now - start >= 2000)
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
        std::fs::remove_dir_all(root).unwrap();
    }
}
