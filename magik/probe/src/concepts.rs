// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use mister_magik_visual_concepts::{Preset, Scene};
use std::time::{Duration, Instant};
fn configure_card_worker() {
    use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
    apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRenderer);
}
pub struct Concepts {
    pub scene: Option<Scene>,
    pub name: String,
    pub preset: Preset,
    pub paused: bool,
    pub dirty: bool,
    pub measure: bool,
    pub measure_duration_ms: u64,
    pub generation: i32,
    pub advance_next: bool,
    animation_at: Option<Instant>,
    pub stop_at: Option<Duration>,
    pub error: Option<String>,
    pub preparation_ms: u64,
    profile_preparation: bool,
    benchmark_preparation: bool,
    pub preparation_benchmark: bool,
    pub startup_started: Option<Instant>,
    pub preparation_profile: serde_json::Value,
    width: usize,
    height: usize,
}
impl Concepts {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            scene: None,
            name: String::new(),
            preset: Preset::Default,
            paused: false,
            dirty: false,
            measure: false,
            measure_duration_ms: 30_000,
            generation: 0,
            advance_next: false,
            animation_at: None,
            stop_at: None,
            error: None,
            preparation_ms: 0,
            profile_preparation: false,
            benchmark_preparation: false,
            preparation_benchmark: false,
            startup_started: None,
            preparation_profile: serde_json::Value::Null,
            width,
            height,
        }
    }
    pub fn select(&mut self, name: &str, preset: Preset) {
        self.animation_at = None;
        self.stop_at = None;
        self.generation = self.generation.wrapping_add(1);
        let previous_time = self.scene.as_ref().map(Scene::elapsed);
        let keep_time =
            self.name == name && mister_magik_visual_concepts::RENDER_LABS.contains(&name);
        // Release old caches before preparing a replacement.
        self.scene = None;
        let worker_setup = if mister_magik_visual_concepts::RENDER_LABS.contains(&name) {
            use mister_magik_catalog::runtime_thread::{
                RuntimeThreadRole, apply_runtime_thread_policy,
            };
            apply_runtime_thread_policy(RuntimeThreadRole::LauncherUi);
            Some(configure_card_worker as fn())
        } else {
            None
        };
        let profiling = std::mem::take(&mut self.profile_preparation);
        let benchmarking = std::mem::take(&mut self.benchmark_preparation);
        self.preparation_benchmark = benchmarking || profiling;
        self.startup_started = None;
        self.preparation_profile = serde_json::Value::Null;
        let sampler = if profiling {
            match mister_magik_tooling_support::CpuProfile::start() {
                Ok(Some(sampler)) => Some(sampler),
                Ok(None) => {
                    self.error =
                        Some("preparation profiling requires a managed profile session".into());
                    return;
                }
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            }
        } else {
            None
        };
        if profiling {
            mister_magik_framebuffer_scenes::launcher_profile::enable_wall_time();
        }
        let mut resources =
            mister_magik_tooling_support::measurement::PresentationMetrics::default();
        if profiling {
            crate::measurement::resources(&mut resources);
        }
        let cpu_start = resources.process_cpu_us;
        let preparation_started = std::time::Instant::now();
        self.startup_started = self.preparation_benchmark.then_some(preparation_started);
        match Scene::new_with_worker_setup(name, preset, self.width, self.height, worker_setup) {
            Ok(mut scene) => {
                self.preparation_ms = preparation_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                if profiling {
                    crate::measurement::resources(&mut resources);
                    let stages = mister_magik_framebuffer_scenes::launcher_profile::take();
                    mister_magik_framebuffer_scenes::launcher_profile::disable();
                    let sampled = sampler.unwrap().finish();
                    self.preparation_profile = serde_json::json!({
                        "scope": "cold-scene-preparation", "complete": sampled.is_ok(),
                        "sampler_hz": 99, "preparation_ms": self.preparation_ms,
                        "process_cpu_us": cpu_start.zip(resources.process_cpu_us).map(|(a,b)| b.saturating_sub(a)),
                        "renderer": stages,
                    });
                    if let Err(error) = sampled {
                        self.error = Some(error);
                        return;
                    }
                }
                if keep_time && let Some(time) = previous_time {
                    scene.advance(time);
                }
                self.scene = Some(scene);
                self.advance_next = false;
                self.name = name.into();
                self.preset = preset;
                if !keep_time {
                    self.paused = false;
                }
                if self.preparation_benchmark {
                    self.paused = true;
                }
                self.dirty = true;
                self.error = None;
                if let Some(root) = std::env::var_os("MISTER_MAGIK2_STATE_ROOT") {
                    let path = std::path::PathBuf::from(root).join("mini-concept.json");
                    let value = serde_json::json!({"name":name,"preset":preset.name()});
                    if let Err(error) = std::fs::write(&path, value.to_string()) {
                        self.error = Some(format!("cannot retain concept selection: {error}"));
                    }
                }
            }
            Err(e) => {
                if profiling {
                    mister_magik_framebuffer_scenes::launcher_profile::disable();
                }
                self.error = Some(e);
            }
        }
    }
    /// Live motion follows monotonic time even when a render misses a refresh.
    /// Bookmarks retain deterministic stepping and clamp to the exact pose.
    pub fn advance_frame(&mut self, now: Instant) {
        if self.paused {
            self.animation_at = None;
            return;
        }
        let previous = self.animation_at.replace(now);
        if !self.advance_next {
            return;
        }
        if let Some(scene) = &mut self.scene {
            let delta = if let Some(target) = self.stop_at {
                Duration::from_nanos(16_666_667).min(target.saturating_sub(scene.elapsed()))
            } else {
                previous.map_or(Duration::ZERO, |at| now.saturating_duration_since(at))
            };
            scene.advance(delta);
        }
    }
    pub fn action(&mut self, action: &str) {
        if action == "bench-preparation" {
            self.benchmark_preparation = true;
            return;
        }
        if action == "profile-preparation" {
            self.profile_preparation = true;
            return;
        }
        self.animation_at = None;
        if let Some(bookmark) = action.strip_prefix("capture-") {
            let (midpoint, boundary) = match self.name.as_str() {
                "launcher-cards" => (210, 420),
                "arcade-transition" | "settings-transition" => (500, 1000),
                "light-sweep" => (1500, 3000),
                "pixel-dissolve" => (1300, 3200),
                "starfield" => (4096, 8192),
                "texture-tunnel" => (4000, 8000),
                "raster-waves" => (1024, 2048),
                _ => (100, 7680),
            };
            let target = Duration::from_millis(match bookmark {
                "initial" => 0,
                "midpoint" => midpoint,
                "boundary" => boundary,
                "handoff" if self.name == "settings-transition" => 180,
                "return-handoff" if self.name == "settings-transition" => 2020,
                _ => {
                    self.error = Some("unknown capture bookmark".into());
                    return;
                }
            });
            if let Some(scene) = &mut self.scene {
                if target <= scene.elapsed() {
                    scene.reset();
                    self.advance_next = false;
                }
                self.stop_at = Some(target);
                self.paused = false;
                self.dirty = true;
            }
            return;
        }
        self.animation_at = None;
        self.stop_at = None;
        match action {
            "pause" => self.paused = true,
            "resume" => self.paused = false,
            "restart" | "measure" | "measure-short" => {
                if let Some(s) = &mut self.scene {
                    s.reset();
                    self.advance_next = false;
                    self.dirty = true;
                }
                if action.starts_with("measure") {
                    self.paused = false;
                }
                self.measure = action.starts_with("measure");
                self.measure_duration_ms = if action == "measure-short" {
                    10_000
                } else {
                    30_000
                };
            }
            "step" => {
                self.paused = true;
                if let Some(s) = &mut self.scene {
                    s.advance(Duration::from_nanos(16_666_667));
                    self.dirty = true;
                }
            }
            _ => self.error = Some(format!("unknown action: {action}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rendering_variant_switch_preserves_paused_frame() {
        let mut c = Concepts::new(960, 540);
        c.select("launcher-cards", Preset::Default);
        c.action("pause");
        c.action("step");
        let before = c.scene.as_ref().unwrap().elapsed();
        let generation = c.generation;
        c.select("launcher-cards", Preset::Rgb888);
        assert!(c.paused);
        assert_eq!(c.scene.as_ref().unwrap().elapsed(), before);
        assert_ne!(c.generation, generation);
        c.action("restart");
        assert!(c.paused);
        assert_eq!(c.scene.as_ref().unwrap().elapsed(), Duration::ZERO);
        c.select("arcade-transition", Preset::Rgb888);
        assert!(!c.paused);
        assert_eq!(c.scene.as_ref().unwrap().elapsed(), Duration::ZERO);
    }
    #[test]
    fn pause_step_restart_preserve_an_explicit_timeline() {
        let mut c = Concepts::new(960, 540);
        c.select("diagnostic", Preset::Default);
        c.scene.as_mut().unwrap().render().unwrap();
        c.advance_next = true;
        c.action("pause");
        c.action("step");
        assert_eq!(
            c.scene.as_ref().unwrap().elapsed(),
            Duration::from_nanos(16_666_667)
        );
        assert!(c.paused && c.dirty);
        c.action("restart");
        assert_eq!(c.scene.as_ref().unwrap().elapsed(), Duration::ZERO);
        assert!(c.paused && !c.advance_next);
        c.action("measure");
        assert!(c.measure && !c.paused);
        assert_eq!(c.measure_duration_ms, 30_000);
        c.action("measure-short");
        assert!(c.measure && !c.paused);
        assert_eq!(c.measure_duration_ms, 10_000);
        let generation = c.generation;
        c.select("diagnostic", Preset::Reduced);
        assert_ne!(c.generation, generation);
        assert_eq!(c.preset, Preset::Reduced);
    }
    #[test]
    fn live_clock_follows_time_without_including_paused_time() {
        let mut c = Concepts::new(960, 540);
        c.select("diagnostic", Preset::Default);
        let start = Instant::now();
        c.advance_frame(start);
        c.advance_next = true;
        c.advance_frame(start + Duration::from_millis(47));
        assert_eq!(
            c.scene.as_ref().unwrap().elapsed(),
            Duration::from_millis(47)
        );
        c.action("pause");
        c.advance_frame(start + Duration::from_secs(5));
        c.action("resume");
        c.advance_frame(start + Duration::from_secs(6));
        c.advance_frame(start + Duration::from_millis(6019));
        assert_eq!(
            c.scene.as_ref().unwrap().elapsed(),
            Duration::from_millis(66)
        );
        c.action("capture-midpoint");
        for frame in 0..200 {
            c.advance_frame(start + Duration::from_secs(7) + Duration::from_millis(frame * 29));
        }
        assert_eq!(
            c.scene.as_ref().unwrap().elapsed(),
            Duration::from_millis(100)
        );
    }
}
