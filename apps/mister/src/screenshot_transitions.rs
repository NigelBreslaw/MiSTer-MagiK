// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Screenshot preview transition sequencing.

use std::time::Duration;

#[cfg(mister_experiments)]
use crate::experiments::preview_transitions as experiment_preview_transitions;
use crate::preview_state::PreviewRawTransitionFrame;
use mister_magik_fb::preview_transition::{PreviewTransitionController, transition_duration_ratio};

const DEFAULT_PREVIEW_TRANSITION_MS: u64 = 130;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewTransitionEffect {
    Fade,
    #[cfg(mister_experiments)]
    Wipe,
    #[cfg(mister_experiments)]
    Slide,
    #[cfg(mister_experiments)]
    Zoom,
    #[cfg(mister_experiments)]
    Scanline,
    #[cfg(mister_experiments)]
    Checker,
    #[cfg(mister_experiments)]
    Dissolve,
    #[cfg(mister_experiments)]
    CrtBeamWipe,
    #[cfg(mister_experiments)]
    MosaicResolve,
    #[cfg(mister_experiments)]
    CopperBars,
    #[cfg(mister_experiments)]
    VenetianBlinds,
    #[cfg(mister_experiments)]
    BarnDoor,
    #[cfg(mister_experiments)]
    Iris,
    #[cfg(mister_experiments)]
    ClockWipe,
    #[cfg(mister_experiments)]
    SpriteStrips,
    #[cfg(mister_experiments)]
    StarfieldWarp,
    #[cfg(mister_experiments)]
    VectorRedraw,
    #[cfg(mister_experiments)]
    PaletteCycle,
    #[cfg(mister_experiments)]
    RasterTear,
    #[cfg(mister_experiments)]
    TileLoader,
    #[cfg(mister_experiments)]
    VenetianCopper,
    #[cfg(mister_experiments)]
    AttributeFlash,
    #[cfg(mister_experiments)]
    TecTec,
    #[cfg(mister_experiments)]
    Linecrunch,
    #[cfg(mister_experiments)]
    RacingBeam,
    #[cfg(mister_experiments)]
    SpriteMultiplex,
    #[cfg(mister_experiments)]
    RowScrollParallax,
    #[cfg(mister_experiments)]
    SuperScalerPop,
    #[cfg(mister_experiments)]
    MaskBlit,
    #[cfg(mister_experiments)]
    PhosphorDecay,
    #[cfg(mister_experiments)]
    PlasmaMask,
    #[cfg(mister_experiments)]
    MoireRings,
    #[cfg(mister_experiments)]
    KefrensCurtain,
}

impl PreviewTransitionEffect {
    #[cfg(not(mister_experiments))]
    pub const PRODUCTION: [Self; 1] = [Self::Fade];

    pub fn all() -> &'static [Self] {
        #[cfg(mister_experiments)]
        {
            experiment_preview_transitions::all()
        }
        #[cfg(not(mister_experiments))]
        {
            &Self::PRODUCTION
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fade => "fade",
            #[cfg(mister_experiments)]
            other => experiment_preview_transitions::label(other),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().replace('_', "-").as_str() {
            "fade" | "crossfade" | "cross-fade" => Some(Self::Fade),
            #[cfg(mister_experiments)]
            other => experiment_preview_transitions::parse(other),
            #[cfg(not(mister_experiments))]
            _ => None,
        }
    }

    #[cfg_attr(not(mister_experiments), allow(dead_code))]
    pub fn labels() -> String {
        Self::all()
            .iter()
            .map(|effect| effect.label())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewTransitionTrace {
    pub effect: PreviewTransitionEffect,
    pub progress: f32,
    pub active: bool,
    pub fade: PreviewFadeTrace,
}

impl Default for PreviewTransitionTrace {
    fn default() -> Self {
        Self {
            effect: PreviewTransitionEffect::Fade,
            progress: 1.0,
            active: false,
            fade: PreviewFadeTrace::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewFadeTrace {
    pub wall_us: u64,
    pub cpu_us: u64,
    pub pixels: u32,
    pub rows: u32,
    pub path: PreviewFadePath,
    pub alpha_bucket: u8,
}

impl Default for PreviewFadeTrace {
    fn default() -> Self {
        Self {
            wall_us: 0,
            cpu_us: 0,
            pixels: 0,
            rows: 0,
            path: PreviewFadePath::None,
            alpha_bucket: 0,
        }
    }
}

impl PreviewFadeTrace {
    pub fn label(self) -> &'static str {
        self.path.label()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewFadePath {
    None,
    Cut,
    SameGeometry,
    SingleBlack,
    Rows,
    ScaledSample,
    Empty,
}

impl PreviewFadePath {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Cut => "cut",
            Self::SameGeometry => "same_geometry",
            Self::SingleBlack => "single_black",
            Self::Rows => "rows",
            Self::ScaledSample => "scaled_sample",
            Self::Empty => "empty",
        }
    }
}

pub struct PreviewTransitionDemo {
    effects: Vec<PreviewTransitionEffect>,
    picker_index: Option<usize>,
    pub segment: Duration,
    pub duration: Duration,
    timeline: PreviewTransitionController<PreviewTransitionEffect>,
    label_overlay: bool,
}

impl PreviewTransitionDemo {
    pub fn disabled() -> Self {
        Self {
            effects: vec![PreviewTransitionEffect::Fade],
            picker_index: None,
            segment: Duration::from_secs(1),
            duration: Duration::from_millis(DEFAULT_PREVIEW_TRANSITION_MS),
            timeline: PreviewTransitionController::default(),
            label_overlay: false,
        }
    }

    pub fn from_env() -> Self {
        let spec = std::env::var("MISTER_PREVIEW_TRANSITION").unwrap_or_default();
        let mut effects = Vec::new();
        let trimmed = spec.trim();
        let picker_enabled = transition_picker_enabled();
        let label_overlay = picker_enabled || !trimmed.is_empty();
        let use_all = picker_enabled && trimmed.is_empty();
        #[cfg(mister_experiments)]
        let use_all = use_all
            || trimmed.eq_ignore_ascii_case("mega")
            || trimmed.eq_ignore_ascii_case("all")
            || trimmed.eq_ignore_ascii_case("demo");
        if use_all {
            effects.extend(PreviewTransitionEffect::all());
        } else if !trimmed.is_empty() {
            for part in trimmed
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
            {
                if let Some(effect) = PreviewTransitionEffect::parse(part) {
                    effects.push(effect);
                } else {
                    crate::ui_errln!(
                        "ui: unknown MISTER_PREVIEW_TRANSITION effect {part:?}; use `mister-magik-fb preview-transitions` for labels"
                    );
                }
            }
        }
        if effects.is_empty() {
            effects.push(PreviewTransitionEffect::Fade);
        }
        let picker_index = picker_enabled.then_some(
            effects
                .iter()
                .position(|effect| *effect == PreviewTransitionEffect::Fade)
                .unwrap_or(0),
        );
        let segment_secs = std::env::var("MISTER_PREVIEW_TRANSITION_SEGMENT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5)
            .max(1);
        let duration_ms = std::env::var("MISTER_PREVIEW_TRANSITION_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PREVIEW_TRANSITION_MS)
            .clamp(1, 2_000);
        Self {
            effects,
            picker_index,
            segment: Duration::from_secs(segment_secs),
            duration: Duration::from_millis(duration_ms),
            timeline: PreviewTransitionController::default(),
            label_overlay,
        }
    }

    pub fn labels(&self) -> String {
        self.effects
            .iter()
            .map(|effect| effect.label())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn current_effect(&self, elapsed: Duration) -> PreviewTransitionEffect {
        if self.effects.is_empty() {
            return PreviewTransitionEffect::Fade;
        }
        if let Some(idx) = self.picker_index {
            return self.effects[idx.min(self.effects.len() - 1)];
        }
        let segment_us = self.segment.as_micros().max(1);
        let idx = ((elapsed.as_micros() / segment_us) as usize) % self.effects.len();
        self.effects[idx]
    }

    pub fn picker_enabled(&self) -> bool {
        self.picker_index.is_some()
    }

    pub fn current_label(&self, elapsed: Duration) -> &'static str {
        self.current_effect(elapsed).label()
    }

    pub fn cycle_picker(&mut self, delta: isize) -> bool {
        let Some(idx) = self.picker_index else {
            return false;
        };
        if self.effects.is_empty() {
            return false;
        }
        let len = self.effects.len() as isize;
        let next = (idx as isize + delta).rem_euclid(len) as usize;
        if next == idx {
            return false;
        }
        self.picker_index = Some(next);
        self.timeline.reset();
        true
    }

    pub fn label_overlay_enabled(&self) -> bool {
        self.label_overlay
    }

    pub fn update(
        &mut self,
        frame: Option<&PreviewRawTransitionFrame<'_>>,
        elapsed: Duration,
    ) -> PreviewTransitionTrace {
        let scheduled_effect = self.current_effect(elapsed);
        let duration = frame.map_or(self.duration, |frame| {
            transition_duration_ratio(
                self.duration,
                frame.duration_numerator,
                frame.duration_denominator,
            )
        });
        let state = self.timeline.update(
            frame.map(|frame| frame.transition_id),
            frame.is_some_and(|frame| frame.previous.is_some()),
            scheduled_effect,
            duration,
            elapsed,
        );
        PreviewTransitionTrace {
            effect: state.effect,
            progress: state.progress,
            active: state.active,
            fade: PreviewFadeTrace::default(),
        }
    }
}

fn transition_picker_enabled() -> bool {
    std::env::var("MISTER_PREVIEW_TRANSITION_PICKER")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_state::{PreviewRawFrame, PreviewRawPixels};

    fn transition_demo(duration: Duration) -> PreviewTransitionDemo {
        PreviewTransitionDemo {
            effects: vec![PreviewTransitionEffect::Fade],
            picker_index: None,
            segment: Duration::from_secs(5),
            duration,
            timeline: PreviewTransitionController::default(),
            label_overlay: false,
        }
    }

    fn transition_frame_with_ratio(
        transition_id: u64,
        duration_numerator: u32,
        duration_denominator: u32,
    ) -> PreviewRawTransitionFrame<'static> {
        let frame = || PreviewRawFrame {
            pixels: PreviewRawPixels::Empty,
            source_w: 1,
            source_h: 1,
            display_w: 1,
            display_h: 1,
        };
        PreviewRawTransitionFrame {
            previous: Some(frame()),
            current: frame(),
            transition_id,
            duration_numerator,
            duration_denominator,
        }
    }

    fn transition_frame(transition_id: u64) -> PreviewRawTransitionFrame<'static> {
        transition_frame_with_ratio(transition_id, 1, 1)
    }

    #[test]
    fn rapid_retargets_do_not_restart_visible_progress() {
        let mut demo = transition_demo(Duration::from_millis(200));

        assert_eq!(
            demo.update(Some(&transition_frame(1)), Duration::ZERO)
                .progress,
            0.0
        );
        let halfway = demo.update(Some(&transition_frame(2)), Duration::from_millis(100));
        assert!(halfway.active);
        assert_eq!(halfway.progress, 0.5);
        let complete = demo.update(Some(&transition_frame(3)), Duration::from_millis(200));
        assert!(complete.active);
        assert_eq!(complete.progress, 1.0);
        let still_complete = demo.update(Some(&transition_frame(4)), Duration::from_millis(250));
        assert!(still_complete.active);
        assert_eq!(still_complete.progress, 1.0);

        let after_quiet = demo.update(Some(&transition_frame(5)), Duration::from_millis(451));
        assert!(after_quiet.active);
        assert_eq!(after_quiet.progress, 0.0);
    }

    #[test]
    fn completed_chain_requests_only_one_final_present() {
        let mut demo = transition_demo(Duration::from_millis(200));

        demo.update(Some(&transition_frame(1)), Duration::ZERO);
        let complete = demo.update(Some(&transition_frame(1)), Duration::from_millis(200));
        assert!(complete.active);
        assert_eq!(complete.progress, 1.0);
        let settled = demo.update(Some(&transition_frame(1)), Duration::from_millis(201));
        assert!(!settled.active);
        assert_eq!(settled.progress, 1.0);
    }

    #[test]
    fn rapid_retargets_keep_the_initial_effective_duration() {
        let mut demo = transition_demo(Duration::from_millis(130));

        demo.update(
            Some(&transition_frame_with_ratio(1, 63, 130)),
            Duration::ZERO,
        );
        let turbo = demo.update(
            Some(&transition_frame_with_ratio(2, 63, 130)),
            Duration::from_micros(31_500),
        );
        assert_eq!(turbo.progress, 0.5);
        let normal_retarget = demo.update(
            Some(&transition_frame_with_ratio(3, 1, 1)),
            Duration::from_micros(47_250),
        );
        assert_eq!(normal_retarget.progress, 0.75);
    }

    #[cfg(not(mister_experiments))]
    #[test]
    fn production_transitions_are_fade_only() {
        assert_eq!(PreviewTransitionEffect::labels(), "fade");
        assert_eq!(
            PreviewTransitionEffect::all(),
            &[PreviewTransitionEffect::Fade]
        );
        assert_eq!(DEFAULT_PREVIEW_TRANSITION_MS, 130);
    }

    #[test]
    fn disabled_transition_has_no_picker_or_label_overlay() {
        let transition = PreviewTransitionDemo::disabled();

        assert!(!transition.picker_enabled());
        assert!(!transition.label_overlay_enabled());
    }

    #[cfg(not(mister_experiments))]
    #[test]
    fn production_parser_rejects_bench_only_effects() {
        assert_eq!(PreviewTransitionEffect::parse("cut"), None);
        assert_eq!(PreviewTransitionEffect::parse("off"), None);
        assert_eq!(
            PreviewTransitionEffect::parse("fade"),
            Some(PreviewTransitionEffect::Fade)
        );
        assert_eq!(PreviewTransitionEffect::parse("wipe"), None);
        assert_eq!(PreviewTransitionEffect::parse("mega"), None);
    }

    #[test]
    fn transition_duration_ratio_sets_exact_turbo_fade() {
        assert_eq!(
            transition_duration_ratio(Duration::from_millis(130), 63, 130),
            Duration::from_millis(63)
        );
        assert_eq!(
            transition_duration_ratio(Duration::from_millis(130), 1, 1),
            Duration::from_millis(130)
        );
        assert_eq!(
            transition_duration_ratio(Duration::from_millis(130), 0, 0),
            Duration::from_millis(130)
        );
    }

    #[cfg(mister_experiments)]
    #[test]
    fn bench_transitions_keep_experimental_effects() {
        assert!(PreviewTransitionEffect::all().contains(&PreviewTransitionEffect::Wipe));
        assert_eq!(
            PreviewTransitionEffect::parse("wipe"),
            Some(PreviewTransitionEffect::Wipe)
        );
        assert!(PreviewTransitionEffect::labels().contains("kefrens-curtain"));
    }
}
