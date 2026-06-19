//! Screenshot preview transition sequencing.

use std::time::Duration;

use crate::preview_state::PreviewRawTransitionFrame;

const DEFAULT_PREVIEW_TRANSITION_MS: u64 = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewTransitionEffect {
    Cut,
    Fade,
    #[cfg(mister_bench_scenes)]
    Wipe,
    #[cfg(mister_bench_scenes)]
    Slide,
    #[cfg(mister_bench_scenes)]
    Zoom,
    #[cfg(mister_bench_scenes)]
    Scanline,
    #[cfg(mister_bench_scenes)]
    Checker,
    #[cfg(mister_bench_scenes)]
    Dissolve,
    #[cfg(mister_bench_scenes)]
    CrtBeamWipe,
    #[cfg(mister_bench_scenes)]
    MosaicResolve,
    #[cfg(mister_bench_scenes)]
    CopperBars,
    #[cfg(mister_bench_scenes)]
    VenetianBlinds,
    #[cfg(mister_bench_scenes)]
    BarnDoor,
    #[cfg(mister_bench_scenes)]
    Iris,
    #[cfg(mister_bench_scenes)]
    ClockWipe,
    #[cfg(mister_bench_scenes)]
    SpriteStrips,
    #[cfg(mister_bench_scenes)]
    StarfieldWarp,
    #[cfg(mister_bench_scenes)]
    VectorRedraw,
    #[cfg(mister_bench_scenes)]
    PaletteCycle,
    #[cfg(mister_bench_scenes)]
    RasterTear,
    #[cfg(mister_bench_scenes)]
    TileLoader,
    #[cfg(mister_bench_scenes)]
    VenetianCopper,
    #[cfg(mister_bench_scenes)]
    AttributeFlash,
    #[cfg(mister_bench_scenes)]
    TecTec,
    #[cfg(mister_bench_scenes)]
    Linecrunch,
    #[cfg(mister_bench_scenes)]
    RacingBeam,
    #[cfg(mister_bench_scenes)]
    SpriteMultiplex,
    #[cfg(mister_bench_scenes)]
    RowScrollParallax,
    #[cfg(mister_bench_scenes)]
    SuperScalerPop,
    #[cfg(mister_bench_scenes)]
    MaskBlit,
    #[cfg(mister_bench_scenes)]
    PhosphorDecay,
    #[cfg(mister_bench_scenes)]
    PlasmaMask,
    #[cfg(mister_bench_scenes)]
    MoireRings,
    #[cfg(mister_bench_scenes)]
    KefrensCurtain,
}

impl PreviewTransitionEffect {
    #[cfg(not(mister_bench_scenes))]
    pub(crate) const PRODUCTION: [Self; 2] = [Self::Cut, Self::Fade];

    #[cfg(mister_bench_scenes)]
    pub(crate) const MEGA: [Self; 34] = [
        Self::Cut,
        Self::Fade,
        Self::Wipe,
        Self::Slide,
        Self::Zoom,
        Self::Scanline,
        Self::Checker,
        Self::Dissolve,
        Self::CrtBeamWipe,
        Self::MosaicResolve,
        Self::CopperBars,
        Self::VenetianBlinds,
        Self::BarnDoor,
        Self::Iris,
        Self::ClockWipe,
        Self::SpriteStrips,
        Self::StarfieldWarp,
        Self::VectorRedraw,
        Self::PaletteCycle,
        Self::RasterTear,
        Self::TileLoader,
        Self::VenetianCopper,
        Self::AttributeFlash,
        Self::TecTec,
        Self::Linecrunch,
        Self::RacingBeam,
        Self::SpriteMultiplex,
        Self::RowScrollParallax,
        Self::SuperScalerPop,
        Self::MaskBlit,
        Self::PhosphorDecay,
        Self::PlasmaMask,
        Self::MoireRings,
        Self::KefrensCurtain,
    ];

    pub(crate) fn all() -> &'static [Self] {
        #[cfg(mister_bench_scenes)]
        {
            &Self::MEGA
        }
        #[cfg(not(mister_bench_scenes))]
        {
            &Self::PRODUCTION
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cut => "cut",
            Self::Fade => "fade",
            #[cfg(mister_bench_scenes)]
            Self::Wipe => "wipe",
            #[cfg(mister_bench_scenes)]
            Self::Slide => "slide",
            #[cfg(mister_bench_scenes)]
            Self::Zoom => "zoom",
            #[cfg(mister_bench_scenes)]
            Self::Scanline => "scanline",
            #[cfg(mister_bench_scenes)]
            Self::Checker => "checker",
            #[cfg(mister_bench_scenes)]
            Self::Dissolve => "dissolve",
            #[cfg(mister_bench_scenes)]
            Self::CrtBeamWipe => "crt-beam-wipe",
            #[cfg(mister_bench_scenes)]
            Self::MosaicResolve => "mosaic-resolve",
            #[cfg(mister_bench_scenes)]
            Self::CopperBars => "copper-bars",
            #[cfg(mister_bench_scenes)]
            Self::VenetianBlinds => "venetian-blinds",
            #[cfg(mister_bench_scenes)]
            Self::BarnDoor => "barn-door",
            #[cfg(mister_bench_scenes)]
            Self::Iris => "iris",
            #[cfg(mister_bench_scenes)]
            Self::ClockWipe => "clock-wipe",
            #[cfg(mister_bench_scenes)]
            Self::SpriteStrips => "sprite-strips",
            #[cfg(mister_bench_scenes)]
            Self::StarfieldWarp => "starfield-warp",
            #[cfg(mister_bench_scenes)]
            Self::VectorRedraw => "vector-redraw",
            #[cfg(mister_bench_scenes)]
            Self::PaletteCycle => "palette-cycle",
            #[cfg(mister_bench_scenes)]
            Self::RasterTear => "raster-tear",
            #[cfg(mister_bench_scenes)]
            Self::TileLoader => "tile-loader",
            #[cfg(mister_bench_scenes)]
            Self::VenetianCopper => "venetian-copper",
            #[cfg(mister_bench_scenes)]
            Self::AttributeFlash => "attribute-flash",
            #[cfg(mister_bench_scenes)]
            Self::TecTec => "tec-tec",
            #[cfg(mister_bench_scenes)]
            Self::Linecrunch => "linecrunch",
            #[cfg(mister_bench_scenes)]
            Self::RacingBeam => "racing-beam",
            #[cfg(mister_bench_scenes)]
            Self::SpriteMultiplex => "sprite-multiplex",
            #[cfg(mister_bench_scenes)]
            Self::RowScrollParallax => "row-scroll-parallax",
            #[cfg(mister_bench_scenes)]
            Self::SuperScalerPop => "super-scaler-pop",
            #[cfg(mister_bench_scenes)]
            Self::MaskBlit => "mask-blit",
            #[cfg(mister_bench_scenes)]
            Self::PhosphorDecay => "phosphor-decay",
            #[cfg(mister_bench_scenes)]
            Self::PlasmaMask => "plasma-mask",
            #[cfg(mister_bench_scenes)]
            Self::MoireRings => "moire-rings",
            #[cfg(mister_bench_scenes)]
            Self::KefrensCurtain => "kefrens-curtain",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().replace('_', "-").as_str() {
            "cut" | "none" | "off" | "0" | "false" | "no" => Some(Self::Cut),
            "fade" | "crossfade" | "cross-fade" => Some(Self::Fade),
            #[cfg(mister_bench_scenes)]
            "wipe" | "wipe-left" => Some(Self::Wipe),
            #[cfg(mister_bench_scenes)]
            "slide" | "slide-left" => Some(Self::Slide),
            #[cfg(mister_bench_scenes)]
            "zoom" | "pop" => Some(Self::Zoom),
            #[cfg(mister_bench_scenes)]
            "scanline" | "scanlines" | "scan" => Some(Self::Scanline),
            #[cfg(mister_bench_scenes)]
            "checker" | "checkerboard" => Some(Self::Checker),
            #[cfg(mister_bench_scenes)]
            "dissolve" | "noise" => Some(Self::Dissolve),
            #[cfg(mister_bench_scenes)]
            "crt-beam" | "crt-beam-wipe" | "beam" | "beam-wipe" => Some(Self::CrtBeamWipe),
            #[cfg(mister_bench_scenes)]
            "mosaic" | "mosaic-resolve" | "chunky" | "chunky-resolve" => Some(Self::MosaicResolve),
            #[cfg(mister_bench_scenes)]
            "copper" | "copper-bars" => Some(Self::CopperBars),
            #[cfg(mister_bench_scenes)]
            "venetian" | "venetian-blinds" | "blinds" => Some(Self::VenetianBlinds),
            #[cfg(mister_bench_scenes)]
            "barn" | "barn-door" | "barn-doors" => Some(Self::BarnDoor),
            #[cfg(mister_bench_scenes)]
            "iris" | "circle" => Some(Self::Iris),
            #[cfg(mister_bench_scenes)]
            "clock" | "clock-wipe" | "radar" => Some(Self::ClockWipe),
            #[cfg(mister_bench_scenes)]
            "sprite-strips" | "strips" => Some(Self::SpriteStrips),
            #[cfg(mister_bench_scenes)]
            "starfield" | "starfield-warp" | "warp-stars" => Some(Self::StarfieldWarp),
            #[cfg(mister_bench_scenes)]
            "vector" | "vector-redraw" => Some(Self::VectorRedraw),
            #[cfg(mister_bench_scenes)]
            "palette" | "palette-cycle" | "cycle" => Some(Self::PaletteCycle),
            #[cfg(mister_bench_scenes)]
            "raster-tear" | "tear" => Some(Self::RasterTear),
            #[cfg(mister_bench_scenes)]
            "tile-loader" | "tile-load" | "loader" => Some(Self::TileLoader),
            #[cfg(mister_bench_scenes)]
            "venetian-copper" | "copper-blinds" => Some(Self::VenetianCopper),
            #[cfg(mister_bench_scenes)]
            "attribute" | "attribute-flash" | "color-clash" => Some(Self::AttributeFlash),
            #[cfg(mister_bench_scenes)]
            "tec-tec" | "tectec" => Some(Self::TecTec),
            #[cfg(mister_bench_scenes)]
            "linecrunch" | "line-crunch" => Some(Self::Linecrunch),
            #[cfg(mister_bench_scenes)]
            "racing-beam" | "race-beam" => Some(Self::RacingBeam),
            #[cfg(mister_bench_scenes)]
            "sprite-multiplex" | "multiplex" => Some(Self::SpriteMultiplex),
            #[cfg(mister_bench_scenes)]
            "row-scroll-parallax" | "parallax" => Some(Self::RowScrollParallax),
            #[cfg(mister_bench_scenes)]
            "super-scaler-pop" | "superscaler" | "scaler-pop" => Some(Self::SuperScalerPop),
            #[cfg(mister_bench_scenes)]
            "mask-blit" | "mask" => Some(Self::MaskBlit),
            #[cfg(mister_bench_scenes)]
            "phosphor" | "phosphor-decay" => Some(Self::PhosphorDecay),
            #[cfg(mister_bench_scenes)]
            "plasma" | "plasma-mask" => Some(Self::PlasmaMask),
            #[cfg(mister_bench_scenes)]
            "moire" | "moire-rings" => Some(Self::MoireRings),
            #[cfg(mister_bench_scenes)]
            "kefrens" | "kefrens-curtain" | "curtain" => Some(Self::KefrensCurtain),
            _ => None,
        }
    }

    pub(crate) fn labels() -> String {
        Self::all()
            .iter()
            .map(|effect| effect.label())
            .collect::<Vec<_>>()
            .join("\n")
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
    picker_index: Option<usize>,
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
        let picker_enabled = transition_picker_enabled();
        let label_overlay = picker_enabled || !trimmed.is_empty();
        let use_all = picker_enabled && trimmed.is_empty();
        #[cfg(mister_bench_scenes)]
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
                    eprintln!(
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
        if let Some(idx) = self.picker_index {
            return self.effects[idx.min(self.effects.len() - 1)];
        }
        let segment_us = self.segment.as_micros().max(1);
        let idx = ((elapsed.as_micros() / segment_us) as usize) % self.effects.len();
        self.effects[idx]
    }

    pub(crate) fn picker_enabled(&self) -> bool {
        self.picker_index.is_some()
    }

    pub(crate) fn current_label(&self, elapsed: Duration) -> &'static str {
        self.current_effect(elapsed).label()
    }

    pub(crate) fn cycle_picker(&mut self, delta: isize) -> bool {
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
        self.active = None;
        self.last_transition_id = u64::MAX;
        true
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

fn transition_progress(elapsed: Duration, duration: Duration) -> f32 {
    let denom = duration.as_secs_f32();
    if denom <= 0.0 {
        return 1.0;
    }
    (elapsed.as_secs_f32() / denom).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_transitions_are_cut_and_fade_only() {
        assert_eq!(PreviewTransitionEffect::labels(), "cut\nfade");
        assert_eq!(
            PreviewTransitionEffect::all(),
            &[PreviewTransitionEffect::Cut, PreviewTransitionEffect::Fade]
        );
    }

    #[test]
    fn production_parser_rejects_bench_only_effects() {
        assert_eq!(
            PreviewTransitionEffect::parse("cut"),
            Some(PreviewTransitionEffect::Cut)
        );
        assert_eq!(
            PreviewTransitionEffect::parse("fade"),
            Some(PreviewTransitionEffect::Fade)
        );
        assert_eq!(PreviewTransitionEffect::parse("wipe"), None);
        assert_eq!(PreviewTransitionEffect::parse("mega"), None);
    }

    #[cfg(mister_bench_scenes)]
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
