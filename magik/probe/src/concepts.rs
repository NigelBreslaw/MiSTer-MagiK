// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use mister_magik_visual_concepts::{Preset, Scene};
use std::time::Duration;
pub struct Concepts {
    pub scene: Option<Scene>,
    pub name: String,
    pub preset: Preset,
    pub paused: bool,
    pub dirty: bool,
    pub measure: bool,
    pub generation: i32,
    pub advance_next: bool,
    pub stop_at: Option<Duration>,
    pub error: Option<String>,
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
            generation: 0,
            advance_next: false,
            stop_at: None,
            error: None,
            width,
            height,
        }
    }
    pub fn select(&mut self, name: &str, preset: Preset) {
        self.stop_at = None;
        self.generation = self.generation.wrapping_add(1);
        // Release old caches and workers before preparing a replacement.
        self.scene = None;
        match Scene::new(name, preset, self.width, self.height) {
            Ok(scene) => {
                self.scene = Some(scene);
                self.advance_next = false;
                self.name = name.into();
                self.preset = preset;
                self.paused = false;
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
            Err(e) => self.error = Some(e),
        }
    }
    pub fn action(&mut self, action: &str) {
        if let Some(bookmark) = action.strip_prefix("capture-") {
            let (midpoint, boundary) = match self.name.as_str() {
                "point-cloud-morph" => (12500, 20000),
                "depth-parallax" => (320, 16000),
                "light-sweep" => (1500, 3000),
                "pixel-dissolve" => (1300, 3200),
                "starfield" => (4096, 8192),
                "palette-aurora" => (6144, 12288),
                "texture-tunnel" => (4000, 8000),
                "raster-waves" => (1024, 2048),
                "wireframe-terrain" => (64000, 128000),
                _ => (100, 7680),
            };
            let target = Duration::from_millis(match bookmark {
                "initial" => 0,
                "midpoint" => midpoint,
                "boundary" => boundary,
                "cabinet" => 5000,
                _ => {
                    self.error = Some("unknown capture bookmark".into());
                    return;
                }
            });
            if let Some(scene) = &mut self.scene {
                if target <= scene.elapsed() {
                    if let Err(error) = scene.reset() {
                        self.error = Some(error);
                        return;
                    }
                    self.advance_next = false;
                }
                self.stop_at = Some(target);
                self.paused = false;
                self.dirty = true;
            }
            return;
        }
        self.stop_at = None;
        match action {
            "pause" => self.paused = true,
            "resume" => self.paused = false,
            "restart" | "measure" => {
                if let Some(s) = &mut self.scene {
                    if let Err(error) = s.reset() {
                        self.error = Some(error);
                        return;
                    }
                    self.advance_next = false;
                    self.dirty = true;
                }
                if action == "measure" {
                    self.paused = false;
                }
                self.measure = action == "measure";
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
        let generation = c.generation;
        c.select("diagnostic", Preset::Reduced);
        assert_ne!(c.generation, generation);
        assert_eq!(c.preset, Preset::Reduced);
    }
}
