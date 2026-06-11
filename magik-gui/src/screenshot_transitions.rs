//! Screenshot preview transition sequencing.

use std::time::Duration;

use crate::preview_state::PreviewRawTransitionFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewTransitionEffect {
    Cut,
    Fade,
    Wipe,
    Slide,
    Zoom,
    Scanline,
    Checker,
    Dissolve,
}

impl PreviewTransitionEffect {
    const MEGA: [Self; 8] = [
        Self::Cut,
        Self::Fade,
        Self::Wipe,
        Self::Slide,
        Self::Zoom,
        Self::Scanline,
        Self::Checker,
        Self::Dissolve,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cut => "cut",
            Self::Fade => "fade",
            Self::Wipe => "wipe",
            Self::Slide => "slide",
            Self::Zoom => "zoom",
            Self::Scanline => "scanline",
            Self::Checker => "checker",
            Self::Dissolve => "dissolve",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().replace('_', "-").as_str() {
            "cut" | "none" | "off" | "0" | "false" | "no" => Some(Self::Cut),
            "fade" | "crossfade" | "cross-fade" => Some(Self::Fade),
            "wipe" | "wipe-left" => Some(Self::Wipe),
            "slide" | "slide-left" => Some(Self::Slide),
            "zoom" | "pop" => Some(Self::Zoom),
            "scanline" | "scanlines" | "scan" => Some(Self::Scanline),
            "checker" | "checkerboard" => Some(Self::Checker),
            "dissolve" | "noise" => Some(Self::Dissolve),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ActivePreviewTransition {
    transition_id: u64,
    effect: PreviewTransitionEffect,
    start_elapsed: Duration,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PreviewTransitionTrace {
    pub(crate) effect: PreviewTransitionEffect,
    pub(crate) progress: f32,
    pub(crate) active: bool,
}

impl Default for PreviewTransitionTrace {
    fn default() -> Self {
        Self {
            effect: PreviewTransitionEffect::Fade,
            progress: 1.0,
            active: false,
        }
    }
}

pub(crate) struct PreviewTransitionDemo {
    effects: Vec<PreviewTransitionEffect>,
    pub(crate) segment: Duration,
    pub(crate) duration: Duration,
    last_transition_id: u64,
    active: Option<ActivePreviewTransition>,
    label_overlay: bool,
}

impl PreviewTransitionDemo {
    pub(crate) fn from_env() -> Self {
        let spec = std::env::var("MISTER_PREVIEW_TRANSITION").unwrap_or_default();
        let mut effects = Vec::new();
        let trimmed = spec.trim();
        let label_overlay = !trimmed.is_empty();
        if trimmed.eq_ignore_ascii_case("mega")
            || trimmed.eq_ignore_ascii_case("all")
            || trimmed.eq_ignore_ascii_case("demo")
        {
            effects.extend(PreviewTransitionEffect::MEGA);
        } else if !trimmed.is_empty() {
            for part in trimmed
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
            {
                if let Some(effect) = PreviewTransitionEffect::parse(part) {
                    effects.push(effect);
                } else {
                    eprintln!(
                        "ui: unknown MISTER_PREVIEW_TRANSITION effect {part:?}; use cut|fade|wipe|slide|zoom|scanline|checker|dissolve|mega"
                    );
                }
            }
        }
        if effects.is_empty() {
            effects.push(PreviewTransitionEffect::Fade);
        }
        let segment_secs = std::env::var("MISTER_PREVIEW_TRANSITION_SEGMENT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(5)
            .max(1);
        let duration_ms = std::env::var("MISTER_PREVIEW_TRANSITION_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(320)
            .clamp(1, 2_000);
        Self {
            effects,
            segment: Duration::from_secs(segment_secs),
            duration: Duration::from_millis(duration_ms),
            last_transition_id: u64::MAX,
            active: None,
            label_overlay,
        }
    }

    pub(crate) fn labels(&self) -> String {
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
        let segment_us = self.segment.as_micros().max(1);
        let idx = ((elapsed.as_micros() / segment_us) as usize) % self.effects.len();
        self.effects[idx]
    }

    pub(crate) fn label_overlay_enabled(&self) -> bool {
        self.label_overlay
    }

    pub(crate) fn update(
        &mut self,
        frame: Option<&PreviewRawTransitionFrame<'_>>,
        elapsed: Duration,
    ) -> PreviewTransitionTrace {
        let scheduled_effect = self.current_effect(elapsed);
        let Some(frame) = frame else {
            self.active = None;
            return PreviewTransitionTrace {
                effect: scheduled_effect,
                progress: 1.0,
                active: false,
            };
        };

        if frame.transition_id != self.last_transition_id {
            self.last_transition_id = frame.transition_id;
            self.active =
                if frame.previous.is_some() && scheduled_effect != PreviewTransitionEffect::Cut {
                    Some(ActivePreviewTransition {
                        transition_id: frame.transition_id,
                        effect: scheduled_effect,
                        start_elapsed: elapsed,
                    })
                } else {
                    None
                };
        }

        if let Some(active) = self.active {
            if active.transition_id == frame.transition_id {
                let progress = transition_progress(
                    elapsed.saturating_sub(active.start_elapsed),
                    self.duration,
                );
                if progress < 1.0 {
                    return PreviewTransitionTrace {
                        effect: active.effect,
                        progress,
                        active: true,
                    };
                }
                self.active = None;
                return PreviewTransitionTrace {
                    effect: active.effect,
                    progress: 1.0,
                    active: true,
                };
            }
            self.active = None;
        }

        PreviewTransitionTrace {
            effect: scheduled_effect,
            progress: 1.0,
            active: false,
        }
    }
}

fn transition_progress(elapsed: Duration, duration: Duration) -> f32 {
    let denom = duration.as_secs_f32();
    if denom <= 0.0 {
        return 1.0;
    }
    (elapsed.as_secs_f32() / denom).clamp(0.0, 1.0)
}
