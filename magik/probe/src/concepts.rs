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
            error: None,
            width,
            height,
        }
    }
    pub fn select(&mut self, name: &str, preset: Preset) {
        match Scene::new(name, preset, self.width, self.height) {
            Ok(scene) => {
                self.scene = Some(scene);
                self.name = name.into();
                self.preset = preset;
                self.paused = false;
                self.dirty = true;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }
    pub fn action(&mut self, action: &str) {
        match action {
            "pause" => self.paused = true,
            "resume" => self.paused = false,
            "restart" | "measure" => {
                if let Some(s) = &mut self.scene {
                    s.reset();
                    self.dirty = true;
                }
                self.paused = false;
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
