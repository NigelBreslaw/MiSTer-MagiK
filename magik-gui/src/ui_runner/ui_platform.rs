// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(crate) struct MisterPlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    pub(crate) start: Instant,
    pub(crate) fixed_time: Option<Rc<Cell<Duration>>>,
}

#[derive(Clone)]
pub(crate) struct AnimationClock {
    fixed_time: Option<Rc<Cell<Duration>>>,
    fixed_step: Duration,
}

impl AnimationClock {
    pub(crate) fn from_env() -> Self {
        match std::env::var("MISTER_ANIMATION_CLOCK")
            .ok()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("fixed60") | Some("fixed-60") | Some("frame")
            | Some("frame-clock") => Self {
                fixed_time: Some(Rc::new(Cell::new(Duration::ZERO))),
                fixed_step: Duration::from_nanos(16_666_667),
            },
            Some("wall") | Some("wall-clock") => Self {
                fixed_time: None,
                fixed_step: Duration::from_nanos(16_666_667),
            },
            other => {
                crate::ui_errln!("ui: unknown MISTER_ANIMATION_CLOCK={other:?}; use wall|fixed60");
                Self {
                    fixed_time: None,
                    fixed_step: Duration::from_nanos(16_666_667),
                }
            }
        }
    }

    pub(crate) fn platform_time(&self) -> Option<Rc<Cell<Duration>>> {
        self.fixed_time.clone()
    }

    #[cfg(any(mister_bench_scenes, all(target_os = "linux", target_arch = "arm")))]
    pub(super) fn label(&self) -> &'static str {
        if self.fixed_time.is_some() {
            "fixed60"
        } else {
            "wall"
        }
    }

    pub(super) fn advance(&self) {
        if let Some(t) = &self.fixed_time {
            t.set(t.get() + self.fixed_step);
        }
    }
}

pub(crate) fn update_slint_animations(animation_clock: &AnimationClock) {
    animation_clock.advance();
    slint::platform::update_timers_and_animations();
}

const PRESENT_DELAY_ENV: &str = "MISTER_FB_PRESENT_DELAY_US";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FrameOrder {
    RenderThenVsync,
    VsyncThenRender,
}

impl FrameOrder {
    pub(super) fn from_env() -> Self {
        match std::env::var("MISTER_FRAME_ORDER")
            .ok()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("render-then-vsync") | Some("render") => Self::RenderThenVsync,
            Some("vsync-then-render") | Some("vsync-first") | Some("vsync") => {
                Self::VsyncThenRender
            }
            other => {
                crate::ui_errln!(
                    "ui: unknown MISTER_FRAME_ORDER={other:?}; use render-then-vsync|vsync-first"
                );
                Self::RenderThenVsync
            }
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::RenderThenVsync => "render-then-vsync",
            Self::VsyncThenRender => "vsync-first",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PresentTiming {
    delay_us: u64,
}

impl PresentTiming {
    pub(super) fn from_env() -> Self {
        let delay_us = std::env::var(PRESENT_DELAY_ENV)
            .ok()
            .and_then(|value| present_delay_from_value(&value));
        Self {
            delay_us: delay_us.unwrap_or(0),
        }
    }

    pub(super) fn delay_us(self) -> u64 {
        self.delay_us
    }

    pub(super) fn wait_until_present_time(self, vsync_done: std::time::Instant) {
        if self.delay_us == 0 {
            return;
        }
        let target = vsync_done + Duration::from_micros(self.delay_us);
        let now = std::time::Instant::now();
        if target > now {
            std::thread::sleep(target - now);
        }
    }
}

fn present_delay_from_value(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u64>() {
        Ok(delay) => Some(delay.min(50_000)),
        Err(_) => {
            crate::ui_errln!(
                "ui: ignoring invalid {PRESENT_DELAY_ENV}={value:?}; expected microseconds"
            );
            None
        }
    }
}

impl Platform for MisterPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> core::time::Duration {
        self.fixed_time
            .as_ref()
            .map(|t| t.get())
            .unwrap_or_else(|| self.start.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_delay_parses_microseconds() {
        assert_eq!(present_delay_from_value("2500"), Some(2500));
        assert_eq!(present_delay_from_value(""), None);
    }

    #[test]
    fn present_delay_clamps_extreme_values() {
        assert_eq!(present_delay_from_value("999999"), Some(50_000));
    }

    #[test]
    fn present_delay_rejects_invalid_text() {
        assert_eq!(present_delay_from_value("later"), None);
    }
}
