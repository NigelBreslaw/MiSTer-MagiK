// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-testable classic arcade/game and Amiga demo text effects.
#![allow(clippy::too_many_arguments)]

use std::time::Instant;

pub use super::camera_effects::pixel_to_rgb888;
use super::camera_effects::{color, synthetic_images, CameraImage, CameraPixel};
use super::render_helpers::{clear, elapsed_us, time};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEffectKind {
    InsertCoinBlinkCadence,
    HighScoreInitialsCursorPulse,
    SineWaveTextScroller,
    PerLetterBounce,
    PerLetterPaletteChase,
    TextZoomFromHorizon,
    LetterTilesSnapIntoGrid,
    LogoShimmerPaletteCycle,
    ScoreCounterRollingDigits,
    ReadyGoSlapBurst,
    TypewriterDialogueReveal,
    VectorStrokeDrawOnText,
    ContinueCountdownPanic,
    TrackballSignatureInitials,
    GrawlixSpeechBubble,
    RasterbarTitleBacking,
    PaletteCycledTextFill,
    PlasmaFilledLogoText,
    VictoryQuoteTextbox,
    ContinueScreenTipTicker,
    FinishHimImpactPrompt,
    NeoGeoBootSloganFlash,
    ExtendLetterBubbles,
    PhraseSpellingBonusMeter,
    PowerupLetterIconPop,
    IntermissionCaptionCard,
    AttractInstructionPages,
    WaveAnnouncementBanner,
    GetReadyVoiceTextSync,
    DotMatrixCreditRoll,
    AmigaCopperbarScrolltext,
    AmigaRainbowRasterTitle,
    AmigaCopperSplitCredits,
    AmigaBlitterBobLetterSwarm,
    AmigaBobPathScrolltext,
    AmigaShadebobWritingText,
    AmigaInfiniteBobGlyphTrail,
    AmigaKefrensBarTextWipe,
    AmigaMoireCircleTitleMask,
    AmigaPlasmaScrolltextFill,
    AmigaKeftalesZoomTexture,
    AmigaRotozoomLogoText,
    AmigaWobblerFlagText,
    AmigaTextureTunnelTextRibbon,
    AmigaVectorLineFontSpin,
    AmigaFilledVectorLogoTurntable,
    AmigaGlenzTransparentText,
    AmigaBlenkMetalTextSweep,
    AmigaRubberGelTextTwist,
    AmigaScrolltextExplodeReassemble,
}

impl TextEffectKind {
    pub const ALL: [Self; 50] = [
        Self::InsertCoinBlinkCadence,
        Self::HighScoreInitialsCursorPulse,
        Self::SineWaveTextScroller,
        Self::PerLetterBounce,
        Self::PerLetterPaletteChase,
        Self::TextZoomFromHorizon,
        Self::LetterTilesSnapIntoGrid,
        Self::LogoShimmerPaletteCycle,
        Self::ScoreCounterRollingDigits,
        Self::ReadyGoSlapBurst,
        Self::TypewriterDialogueReveal,
        Self::VectorStrokeDrawOnText,
        Self::ContinueCountdownPanic,
        Self::TrackballSignatureInitials,
        Self::GrawlixSpeechBubble,
        Self::RasterbarTitleBacking,
        Self::PaletteCycledTextFill,
        Self::PlasmaFilledLogoText,
        Self::VictoryQuoteTextbox,
        Self::ContinueScreenTipTicker,
        Self::FinishHimImpactPrompt,
        Self::NeoGeoBootSloganFlash,
        Self::ExtendLetterBubbles,
        Self::PhraseSpellingBonusMeter,
        Self::PowerupLetterIconPop,
        Self::IntermissionCaptionCard,
        Self::AttractInstructionPages,
        Self::WaveAnnouncementBanner,
        Self::GetReadyVoiceTextSync,
        Self::DotMatrixCreditRoll,
        Self::AmigaCopperbarScrolltext,
        Self::AmigaRainbowRasterTitle,
        Self::AmigaCopperSplitCredits,
        Self::AmigaBlitterBobLetterSwarm,
        Self::AmigaBobPathScrolltext,
        Self::AmigaShadebobWritingText,
        Self::AmigaInfiniteBobGlyphTrail,
        Self::AmigaKefrensBarTextWipe,
        Self::AmigaMoireCircleTitleMask,
        Self::AmigaPlasmaScrolltextFill,
        Self::AmigaKeftalesZoomTexture,
        Self::AmigaRotozoomLogoText,
        Self::AmigaWobblerFlagText,
        Self::AmigaTextureTunnelTextRibbon,
        Self::AmigaVectorLineFontSpin,
        Self::AmigaFilledVectorLogoTurntable,
        Self::AmigaGlenzTransparentText,
        Self::AmigaBlenkMetalTextSweep,
        Self::AmigaRubberGelTextTwist,
        Self::AmigaScrolltextExplodeReassemble,
    ];

    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::InsertCoinBlinkCadence => "insert-coin-blink-cadence",
            Self::HighScoreInitialsCursorPulse => "high-score-initials-cursor-pulse",
            Self::SineWaveTextScroller => "sine-wave-text-scroller",
            Self::PerLetterBounce => "per-letter-bounce",
            Self::PerLetterPaletteChase => "per-letter-palette-chase",
            Self::TextZoomFromHorizon => "text-zoom-from-horizon",
            Self::LetterTilesSnapIntoGrid => "letter-tiles-snap-into-grid",
            Self::LogoShimmerPaletteCycle => "logo-shimmer-palette-cycle",
            Self::ScoreCounterRollingDigits => "score-counter-rolling-digits",
            Self::ReadyGoSlapBurst => "ready-go-slap-burst",
            Self::TypewriterDialogueReveal => "typewriter-dialogue-reveal",
            Self::VectorStrokeDrawOnText => "vector-stroke-draw-on-text",
            Self::ContinueCountdownPanic => "continue-countdown-panic",
            Self::TrackballSignatureInitials => "trackball-signature-initials",
            Self::GrawlixSpeechBubble => "grawlix-speech-bubble",
            Self::RasterbarTitleBacking => "rasterbar-title-backing",
            Self::PaletteCycledTextFill => "palette-cycled-text-fill",
            Self::PlasmaFilledLogoText => "plasma-filled-logo-text",
            Self::VictoryQuoteTextbox => "victory-quote-textbox",
            Self::ContinueScreenTipTicker => "continue-screen-tip-ticker",
            Self::FinishHimImpactPrompt => "finish-him-impact-prompt",
            Self::NeoGeoBootSloganFlash => "neo-geo-boot-slogan-flash",
            Self::ExtendLetterBubbles => "extend-letter-bubbles",
            Self::PhraseSpellingBonusMeter => "phrase-spelling-bonus-meter",
            Self::PowerupLetterIconPop => "powerup-letter-icon-pop",
            Self::IntermissionCaptionCard => "intermission-caption-card",
            Self::AttractInstructionPages => "attract-instruction-pages",
            Self::WaveAnnouncementBanner => "wave-announcement-banner",
            Self::GetReadyVoiceTextSync => "get-ready-voice-text-sync",
            Self::DotMatrixCreditRoll => "dot-matrix-credit-roll",
            Self::AmigaCopperbarScrolltext => "amiga-copperbar-scrolltext",
            Self::AmigaRainbowRasterTitle => "amiga-rainbow-raster-title",
            Self::AmigaCopperSplitCredits => "amiga-copper-split-credits",
            Self::AmigaBlitterBobLetterSwarm => "amiga-blitter-bob-letter-swarm",
            Self::AmigaBobPathScrolltext => "amiga-bob-path-scrolltext",
            Self::AmigaShadebobWritingText => "amiga-shadebob-writing-text",
            Self::AmigaInfiniteBobGlyphTrail => "amiga-infinite-bob-glyph-trail",
            Self::AmigaKefrensBarTextWipe => "amiga-kefrens-bar-text-wipe",
            Self::AmigaMoireCircleTitleMask => "amiga-moire-circle-title-mask",
            Self::AmigaPlasmaScrolltextFill => "amiga-plasma-scrolltext-fill",
            Self::AmigaKeftalesZoomTexture => "amiga-keftales-zoom-texture",
            Self::AmigaRotozoomLogoText => "amiga-rotozoom-logo-text",
            Self::AmigaWobblerFlagText => "amiga-wobbler-flag-text",
            Self::AmigaTextureTunnelTextRibbon => "amiga-texture-tunnel-text-ribbon",
            Self::AmigaVectorLineFontSpin => "amiga-vector-line-font-spin",
            Self::AmigaFilledVectorLogoTurntable => "amiga-filled-vector-logo-turntable",
            Self::AmigaGlenzTransparentText => "amiga-glenz-transparent-text",
            Self::AmigaBlenkMetalTextSweep => "amiga-blenk-metal-text-sweep",
            Self::AmigaRubberGelTextTwist => "amiga-rubber-gel-text-twist",
            Self::AmigaScrolltextExplodeReassemble => "amiga-scrolltext-explode-reassemble",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.to_ascii_lowercase().replace('_', "-");
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.label() == normalized)
    }

    pub fn labels() -> String {
        Self::ALL
            .iter()
            .map(|kind| kind.label())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextEffectFrameStats {
    pub clear_us: u64,
    pub background_us: u64,
    pub projection_us: u64,
    pub image_blit_us: u64,
    pub sprite_us: u64,
    pub post_us: u64,
    pub hud_us: u64,
    pub glyph_count: u64,
    pub glyph_pixels: u64,
    pub tile_count: u64,
    pub vector_segment_count: u64,
    pub bob_count: u64,
    pub palette_step_count: u64,
    pub hidden_glyph_count: u64,
    pub scroll_offset: u64,
}

impl TextEffectFrameStats {
    pub fn draw_us(self) -> u64 {
        self.clear_us
            + self.background_us
            + self.projection_us
            + self.image_blit_us
            + self.sprite_us
            + self.post_us
            + self.hud_us
    }
}

#[derive(Default)]
struct TextCounters {
    glyph_count: u64,
    glyph_pixels: u64,
    tile_count: u64,
    vector_segment_count: u64,
    bob_count: u64,
    palette_step_count: u64,
    hidden_glyph_count: u64,
    scroll_offset: u64,
}

impl TextCounters {
    fn record_glyph(&mut self, pixels: u64) {
        self.glyph_count += 1;
        if pixels == 0 {
            self.hidden_glyph_count += 1;
        } else {
            self.glyph_pixels += pixels;
        }
    }
}

pub struct TextEffectRenderState {
    scratch: Vec<CameraPixel>,
    w: usize,
    h: usize,
}

impl TextEffectRenderState {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            scratch: vec![CameraPixel(0); w * h],
            w,
            h,
        }
    }

    fn resize(&mut self, w: usize, h: usize) {
        if self.w == w && self.h == h {
            return;
        }
        self.scratch.resize(w * h, CameraPixel(0));
        self.w = w;
        self.h = h;
    }
}

pub fn render_text_effect_frame(
    dst: &mut [CameraPixel],
    state: &mut TextEffectRenderState,
    w: usize,
    h: usize,
    images: &[CameraImage],
    kind: TextEffectKind,
    frame: u64,
    hud: Option<&str>,
) -> TextEffectFrameStats {
    assert_eq!(dst.len(), w * h);
    state.resize(w, h);

    let mut stats = TextEffectFrameStats::default();
    let mut counters = TextCounters::default();

    let t = Instant::now();
    clear(dst, color(2, 3, 10));
    stats.clear_us = elapsed_us(t);

    match kind {
        TextEffectKind::InsertCoinBlinkCadence => {
            time(&mut stats.background_us, || {
                draw_arcade_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_insert_coin(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::HighScoreInitialsCursorPulse => {
            time(&mut stats.background_us, || {
                draw_scoreboard(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_initials_cursor(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::SineWaveTextScroller => {
            time(&mut stats.background_us, || {
                draw_star_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_sine_scroller(
                    dst,
                    w,
                    h,
                    frame,
                    "MISTER MAGIK SINE SCROLLER  ",
                    &mut counters,
                )
            });
        }
        TextEffectKind::PerLetterBounce => {
            time(&mut stats.background_us, || {
                draw_grid_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_letter_bounce(dst, w, h, frame, "BONUS ROUND", &mut counters)
            });
        }
        TextEffectKind::PerLetterPaletteChase => {
            time(&mut stats.background_us, || {
                draw_arcade_backdrop(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_palette_chase(dst, w, h, frame, "PLAYER READY", &mut counters)
            });
        }
        TextEffectKind::TextZoomFromHorizon => {
            time(&mut stats.background_us, || {
                draw_horizon_grid(dst, w, h, frame)
            });
            time(&mut stats.projection_us, || {
                render_zoom_from_horizon(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::LetterTilesSnapIntoGrid => {
            time(&mut stats.background_us, || {
                draw_tile_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_tile_snap(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::LogoShimmerPaletteCycle => {
            time(&mut stats.background_us, || {
                draw_rasterbars(dst, w, h, frame, 2)
            });
            time(&mut stats.sprite_us, || {
                render_logo_shimmer(dst, w, h, frame, "MISTER MAGIK", &mut counters)
            });
        }
        TextEffectKind::ScoreCounterRollingDigits => {
            time(&mut stats.background_us, || {
                draw_scoreboard(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_rolling_digits(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::ReadyGoSlapBurst => {
            time(&mut stats.background_us, || {
                draw_arena_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_ready_go(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::TypewriterDialogueReveal => {
            time(&mut stats.background_us, || {
                draw_dialogue_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_typewriter(dst, w, h, frame, "THE CORE IS ONLINE.", &mut counters)
            });
        }
        TextEffectKind::VectorStrokeDrawOnText => {
            time(&mut stats.background_us, || {
                draw_dark_scanlines(dst, w, h, frame)
            });
            time(&mut stats.projection_us, || {
                render_vector_draw(dst, w, h, frame, "VECTOR", &mut counters)
            });
        }
        TextEffectKind::ContinueCountdownPanic => {
            time(&mut stats.background_us, || {
                draw_warning_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_continue_countdown(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::TrackballSignatureInitials => {
            time(&mut stats.background_us, || {
                draw_signature_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_trackball_signature(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::GrawlixSpeechBubble => {
            time(&mut stats.background_us, || {
                draw_comic_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_grawlix(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::RasterbarTitleBacking => {
            time(&mut stats.background_us, || {
                draw_rasterbars(dst, w, h, frame, 4)
            });
            time(&mut stats.sprite_us, || {
                render_center_title(dst, w, h, frame, "RASTER POWER", 4, &mut counters)
            });
        }
        TextEffectKind::PaletteCycledTextFill => {
            time(&mut stats.background_us, || {
                draw_dark_scanlines(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_text_fill(
                    dst,
                    w,
                    h,
                    frame,
                    "COLOR CYCLE",
                    FillMode::Palette,
                    &mut counters,
                )
            });
        }
        TextEffectKind::PlasmaFilledLogoText => {
            time(&mut stats.background_us, || {
                draw_plasma_cells(dst, w, h, frame, 8)
            });
            time(&mut stats.sprite_us, || {
                render_text_fill(dst, w, h, frame, "PLASMA", FillMode::Plasma, &mut counters)
            });
        }
        TextEffectKind::VictoryQuoteTextbox => {
            time(&mut stats.background_us, || {
                draw_arena_backdrop(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_quote_box(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::ContinueScreenTipTicker => {
            time(&mut stats.background_us, || {
                draw_warning_backdrop(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_tip_ticker(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::FinishHimImpactPrompt => {
            time(&mut stats.background_us, || {
                draw_impact_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_finish_prompt(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::NeoGeoBootSloganFlash => {
            time(&mut stats.background_us, || {
                draw_boot_grid(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_boot_slogan(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::ExtendLetterBubbles => {
            time(&mut stats.background_us, || {
                draw_bubble_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_letter_bubbles(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::PhraseSpellingBonusMeter => {
            time(&mut stats.background_us, || {
                draw_scoreboard(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_phrase_meter(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::PowerupLetterIconPop => {
            time(&mut stats.background_us, || {
                draw_grid_backdrop(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_powerup_letters(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::IntermissionCaptionCard => {
            time(&mut stats.background_us, || {
                draw_intermission_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_intermission_card(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AttractInstructionPages => {
            time(&mut stats.background_us, || {
                draw_arcade_backdrop(dst, w, h, frame / 3)
            });
            time(&mut stats.sprite_us, || {
                render_attract_pages(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::WaveAnnouncementBanner => {
            time(&mut stats.background_us, || {
                draw_horizon_grid(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_wave_banner(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::GetReadyVoiceTextSync => {
            time(&mut stats.background_us, || {
                draw_voice_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_voice_sync(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::DotMatrixCreditRoll => {
            time(&mut stats.background_us, || {
                draw_dot_panel(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_dot_matrix_roll(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaCopperbarScrolltext => {
            time(&mut stats.background_us, || {
                draw_rasterbars(dst, w, h, frame, 6)
            });
            time(&mut stats.sprite_us, || {
                render_sine_scroller(
                    dst,
                    w,
                    h,
                    frame,
                    "AMIGA COPPERBAR SCROLLTEXT  ",
                    &mut counters,
                )
            });
        }
        TextEffectKind::AmigaRainbowRasterTitle => {
            time(&mut stats.background_us, || {
                draw_rainbow_rasters(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_center_title(dst, w, h, frame, "RAINBOW RASTERS", 4, &mut counters)
            });
        }
        TextEffectKind::AmigaCopperSplitCredits => {
            time(&mut stats.background_us, || {
                draw_copper_split(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_copper_credits(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaBlitterBobLetterSwarm => {
            time(&mut stats.background_us, || {
                draw_dark_scanlines(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_bob_swarm(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaBobPathScrolltext => {
            time(&mut stats.background_us, || {
                draw_star_backdrop(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_bob_path_text(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaShadebobWritingText => {
            time(&mut stats.background_us, || {
                draw_dark_scanlines(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_shadebob_writing(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaInfiniteBobGlyphTrail => {
            time(&mut stats.background_us, || {
                draw_grid_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_infinite_bob_trail(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaKefrensBarTextWipe => {
            time(&mut stats.background_us, || {
                draw_kefrens_bars(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_kefrens_text(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaMoireCircleTitleMask => {
            time(&mut stats.background_us, || {
                draw_moire_cells(dst, w, h, frame, 5)
            });
            time(&mut stats.sprite_us, || {
                render_text_fill(dst, w, h, frame, "MOIRE", FillMode::Mask, &mut counters)
            });
        }
        TextEffectKind::AmigaPlasmaScrolltextFill => {
            time(&mut stats.background_us, || {
                draw_plasma_cells(dst, w, h, frame, 6)
            });
            time(&mut stats.sprite_us, || {
                render_plasma_scrolltext(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaKeftalesZoomTexture => {
            time(&mut stats.background_us, || draw_keftales(dst, w, h, frame));
            time(&mut stats.projection_us, || {
                render_zoom_from_horizon(dst, w, h, frame + 40, &mut counters)
            });
        }
        TextEffectKind::AmigaRotozoomLogoText => {
            time(&mut stats.background_us, || {
                draw_rotozoom_cells(dst, w, h, frame)
            });
            time(&mut stats.projection_us, || {
                render_roto_text(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaWobblerFlagText => {
            time(&mut stats.background_us, || {
                draw_rainbow_rasters(dst, w, h, frame / 2)
            });
            time(&mut stats.projection_us, || {
                render_wobbler_text(dst, w, h, frame, "FLAG WOBBLER", &mut counters)
            });
        }
        TextEffectKind::AmigaTextureTunnelTextRibbon => {
            time(&mut stats.background_us, || {
                draw_tunnel(dst, w, h, frame, images)
            });
            time(&mut stats.projection_us, || {
                render_tunnel_ribbon(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaVectorLineFontSpin => {
            time(&mut stats.background_us, || {
                draw_dark_scanlines(dst, w, h, frame)
            });
            time(&mut stats.projection_us, || {
                render_vector_spin(dst, w, h, frame, "SPIN", &mut counters)
            });
        }
        TextEffectKind::AmigaFilledVectorLogoTurntable => {
            time(&mut stats.background_us, || {
                draw_boot_grid(dst, w, h, frame / 2)
            });
            time(&mut stats.projection_us, || {
                render_turntable_logo(dst, w, h, frame, &mut counters)
            });
        }
        TextEffectKind::AmigaGlenzTransparentText => {
            time(&mut stats.background_us, || {
                draw_star_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_layered_transparent_text(dst, w, h, frame, "GLENZ", &mut counters)
            });
        }
        TextEffectKind::AmigaBlenkMetalTextSweep => {
            time(&mut stats.background_us, || {
                draw_copper_split(dst, w, h, frame / 2)
            });
            time(&mut stats.sprite_us, || {
                render_text_fill(dst, w, h, frame, "BLENK", FillMode::Metal, &mut counters)
            });
        }
        TextEffectKind::AmigaRubberGelTextTwist => {
            time(&mut stats.background_us, || {
                draw_plasma_cells(dst, w, h, frame / 2, 10)
            });
            time(&mut stats.projection_us, || {
                render_rubber_text(dst, w, h, frame, "RUBBER", &mut counters)
            });
        }
        TextEffectKind::AmigaScrolltextExplodeReassemble => {
            time(&mut stats.background_us, || {
                draw_impact_backdrop(dst, w, h, frame)
            });
            time(&mut stats.sprite_us, || {
                render_scroll_explode(dst, w, h, frame, &mut counters)
            });
        }
    }

    if let Some(text) = hud {
        time(&mut stats.hud_us, || draw_label(dst, w, h, text));
    }

    stats.glyph_count = counters.glyph_count;
    stats.glyph_pixels = counters.glyph_pixels;
    stats.tile_count = counters.tile_count;
    stats.vector_segment_count = counters.vector_segment_count;
    stats.bob_count = counters.bob_count;
    stats.palette_step_count = counters.palette_step_count;
    stats.hidden_glyph_count = counters.hidden_glyph_count;
    stats.scroll_offset = counters.scroll_offset;
    stats
}

pub fn synthetic_text_images(count: usize) -> Vec<CameraImage> {
    synthetic_images(count)
}

fn put(dst: &mut [CameraPixel], w: usize, h: usize, x: isize, y: isize, c: CameraPixel) -> u64 {
    if x >= 0 && y >= 0 {
        let x = x as usize;
        let y = y as usize;
        if x < w && y < h {
            dst[y * w + x] = c;
            return 1;
        }
    }
    0
}

fn fill_rect(
    dst: &mut [CameraPixel],
    screen_w: usize,
    screen_h: usize,
    x: isize,
    y: isize,
    rw: usize,
    rh: usize,
    c: CameraPixel,
) -> u64 {
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + rw as isize).clamp(0, screen_w as isize) as usize;
    let y1 = (y + rh as isize).clamp(0, screen_h as isize) as usize;
    if x1 <= x0 || y1 <= y0 {
        return 0;
    }
    for yy in y0..y1 {
        dst[yy * screen_w + x0..yy * screen_w + x1].fill(c);
    }
    ((x1 - x0) * (y1 - y0)) as u64
}

fn tint_pixel(px: CameraPixel, amount: u8) -> CameraPixel {
    let rgb = pixel_to_rgb888(px);
    let a = amount as u32;
    color(
        (((rgb >> 16) & 255) * a / 255) as u8,
        (((rgb >> 8) & 255) * a / 255) as u8,
        ((rgb & 255) * a / 255) as u8,
    )
}

fn blend(a: CameraPixel, b: CameraPixel, amount: u8) -> CameraPixel {
    let ar = pixel_to_rgb888(a);
    let br = pixel_to_rgb888(b);
    let t = amount as u32;
    let inv = 255 - t;
    color(
        ((((ar >> 16) & 255) * inv + ((br >> 16) & 255) * t) / 255) as u8,
        ((((ar >> 8) & 255) * inv + ((br >> 8) & 255) * t) / 255) as u8,
        (((ar & 255) * inv + (br & 255) * t) / 255) as u8,
    )
}

fn triangle(v: usize) -> u8 {
    let x = v & 255;
    if x < 128 {
        (x * 2) as u8
    } else {
        ((255 - x) * 2) as u8
    }
}

fn wave(v: usize, amplitude: isize) -> isize {
    (triangle(v) as isize - 128) * amplitude / 128
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn palette(idx: usize, frame: u64) -> CameraPixel {
    match (idx + frame as usize / 3) & 7 {
        0 => color(255, 76, 96),
        1 => color(255, 176, 64),
        2 => color(255, 238, 96),
        3 => color(88, 238, 126),
        4 => color(76, 226, 238),
        5 => color(86, 142, 255),
        6 => color(190, 100, 255),
        _ => color(255, 112, 210),
    }
}

fn glyph5x7(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b01010, 0b01010, 0b00100, 0b01010, 0b01010, 0b10001,
        ],
        'Y' => [
            0b10001, 0b01010, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b11100,
        ],
        '-' | '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '/' => [0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0, 0],
        ':' => [0, 0b00100, 0b00100, 0, 0b00100, 0b00100, 0],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100, 0],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '\'' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0],
        '"' => [0b01010, 0b01010, 0b01010, 0, 0, 0, 0],
        '#' => [
            0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
        '$' => [
            0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100,
        ],
        '%' => [0b11001, 0b11010, 0b00100, 0b01000, 0b10110, 0b00110, 0],
        '&' => [
            0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101,
        ],
        '*' => [0, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        '>' => [
            0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000,
        ],
        '<' => [
            0b00001, 0b00010, 0b00100, 0b01000, 0b00100, 0b00010, 0b00001,
        ],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        _ => [
            0b11111, 0b10001, 0b00110, 0b00110, 0b00110, 0b10001, 0b11111,
        ],
    }
}

fn text_width(text: &str, scale: usize) -> usize {
    text.chars().count() * 6 * scale
}

fn draw_text_scaled(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    text: &str,
    x: isize,
    y: isize,
    scale: usize,
    c: CameraPixel,
    counters: &mut TextCounters,
) {
    let scale = scale.max(1);
    let mut cx = x;
    for ch in text.chars() {
        let pixels = draw_glyph_scaled(dst, w, h, ch, cx, y, scale, c);
        if ch != ' ' {
            counters.record_glyph(pixels);
        }
        cx += (scale * 6) as isize;
    }
}

fn draw_text_shadowed(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    text: &str,
    x: isize,
    y: isize,
    scale: usize,
    c: CameraPixel,
    counters: &mut TextCounters,
) {
    let mut shadow = TextCounters::default();
    draw_text_scaled(
        dst,
        w,
        h,
        text,
        x + scale as isize,
        y + scale as isize,
        scale,
        color(0, 0, 0),
        &mut shadow,
    );
    draw_text_scaled(dst, w, h, text, x, y, scale, c, counters);
}

fn draw_glyph_scaled(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    ch: char,
    x: isize,
    y: isize,
    scale: usize,
    c: CameraPixel,
) -> u64 {
    let mut drawn = 0;
    let glyph = glyph5x7(ch);
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) != 0 {
                drawn += fill_rect(
                    dst,
                    w,
                    h,
                    x + (col * scale) as isize,
                    y + (row * scale) as isize,
                    scale,
                    scale,
                    c,
                );
            }
        }
    }
    drawn
}

fn draw_text_palette_rows(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    text: &str,
    x: isize,
    y: isize,
    scale: usize,
    frame: u64,
    mode: FillMode,
    counters: &mut TextCounters,
) {
    let scale = scale.max(1);
    let mut cx = x;
    for (glyph_idx, ch) in text.chars().enumerate() {
        let glyph = glyph5x7(ch);
        let mut glyph_pixels = 0;
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) == 0 {
                    continue;
                }
                let px = cx + (col * scale) as isize;
                let py = y + (row * scale) as isize;
                let c = match mode {
                    FillMode::Palette => palette(row + glyph_idx, frame),
                    FillMode::Plasma => {
                        plasma_color(px as i32 + frame as i32, py as i32 - frame as i32, frame)
                    }
                    FillMode::Mask => {
                        if ((px + py + frame as isize) & 15) < 8 {
                            color(255, 245, 180)
                        } else {
                            color(80, 220, 255)
                        }
                    }
                    FillMode::Metal => {
                        let sweep = ((px - x + frame as isize * 5).rem_euclid(220)) as u8;
                        let bright = 72 + triangle(sweep as usize) / 2;
                        color(bright, bright, bright.saturating_add(38))
                    }
                };
                glyph_pixels += fill_rect(dst, w, h, px, py, scale, scale, c);
                counters.palette_step_count += 1;
            }
        }
        if ch != ' ' {
            counters.record_glyph(glyph_pixels);
        }
        cx += (scale * 6) as isize;
    }
}

#[derive(Clone, Copy)]
enum FillMode {
    Palette,
    Plasma,
    Mask,
    Metal,
}

fn draw_line(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    mut x0: isize,
    mut y0: isize,
    x1: isize,
    y1: isize,
    c: CameraPixel,
) -> u64 {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut drawn = 0;
    loop {
        drawn += put(dst, w, h, x0, y0, c);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
    drawn
}

fn draw_ellipse(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    cx: isize,
    cy: isize,
    rx: isize,
    ry: isize,
    c: CameraPixel,
) -> u64 {
    if rx <= 0 || ry <= 0 {
        return 0;
    }
    let mut drawn = 0;
    let y0 = (cy - ry).max(0) as usize;
    let y1 = (cy + ry).clamp(0, h as isize) as usize;
    for y in y0..y1 {
        let dy = y as isize - cy;
        let span_sq = rx * rx * (ry * ry - dy * dy).max(0) / (ry * ry).max(1);
        let span = int_sqrt(span_sq as u64) as isize;
        drawn += fill_rect(
            dst,
            w,
            h,
            cx - span,
            y as isize,
            (span * 2).max(1) as usize,
            1,
            c,
        );
    }
    drawn
}

fn int_sqrt(v: u64) -> u64 {
    if v == 0 {
        return 0;
    }
    let mut x = v;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

fn plasma_color(x: i32, y: i32, frame: u64) -> CameraPixel {
    let a = triangle((x as isize + frame as isize * 3).unsigned_abs() & 255);
    let b = triangle((y as isize * 2 - frame as isize * 2).unsigned_abs() & 255);
    let c = triangle(((x + y) as isize + frame as isize * 4).unsigned_abs() & 255);
    color(a, b.saturating_add(40), c)
}

fn draw_arcade_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 180 / h.max(1)) as u8;
        let pulse = triangle((frame as usize + y / 3) & 255) / 30;
        dst[y * w..(y + 1) * w].fill(color(4 + pulse, 6 + p / 12, 20 + p / 5));
    }
    for i in 0..80usize {
        let x = (hash(i as u32 * 17) as usize + frame as usize * (1 + i % 3)) % w.max(1);
        let y = (hash(i as u32 * 43) as usize) % h.max(1);
        fill_rect(dst, w, h, x as isize, y as isize, 2, 2, color(60, 100, 150));
    }
}

fn draw_scoreboard(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(0, 4, 12));
    for y in (0..h).step_by(18) {
        let band = ((y / 18 + frame as usize / 12) & 1) as u8;
        dst[y * w..(y + 1) * w].fill(color(12 + band * 10, 24 + band * 12, 42 + band * 16));
    }
    fill_rect(dst, w, h, 0, 0, w, 28.min(h), color(28, 18, 48));
}

fn draw_star_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(1, 2, 12));
    for i in 0..220usize {
        let x = (hash(i as u32 * 71) as usize + frame as usize * (1 + i % 4)) % w.max(1);
        let y = (hash(i as u32 * 29) as usize + frame as usize / (2 + i % 5)) % h.max(1);
        let b = 80 + ((i * 13) & 127) as u8;
        put(dst, w, h, x as isize, y as isize, color(b / 2, b, 255));
    }
}

fn draw_grid_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(color(6, 10 + p / 13, 28 + p / 6));
    }
    let shift = (frame as usize / 2) & 31;
    for x in (0..w + 32).step_by(32) {
        fill_rect(
            dst,
            w,
            h,
            x as isize - shift as isize,
            0,
            2,
            h,
            color(24, 50, 72),
        );
    }
    for y in (h / 2..h).step_by(24) {
        fill_rect(dst, w, h, 0, y as isize, w, 2, color(35, 60, 80));
    }
}

fn draw_horizon_grid(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(if y < h / 2 {
            color(6, 16 + p / 7, 50 + p / 4)
        } else {
            color(20 + p / 12, 18 + p / 16, 32 + p / 18)
        });
    }
    let horizon = h / 2;
    for y in horizon..h {
        let depth = (y - horizon + 1).max(1);
        if ((depth + frame as usize / 2) & 15) == 0 {
            fill_rect(dst, w, h, 0, y as isize, w, 1, color(70, 92, 120));
        }
    }
}

fn draw_tile_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let cell = 28usize;
    for y in (0..h).step_by(cell) {
        for x in (0..w).step_by(cell) {
            let b = ((x / cell + y / cell + frame as usize / 20) & 1) as u8;
            fill_rect(
                dst,
                w,
                h,
                x as isize,
                y as isize,
                cell,
                cell,
                color(10 + b * 12, 22 + b * 14, 44 + b * 18),
            );
        }
    }
}

fn draw_rasterbars(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64, speed: usize) {
    clear(dst, color(0, 0, 8));
    for y in 0..h {
        let band = triangle((y / 2 + frame as usize * speed) & 255);
        let c = color(band, band / 2, 255u8.saturating_sub(band / 2));
        if band > 80 {
            dst[y * w..(y + 1) * w].fill(c);
        }
    }
}

fn draw_arena_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 210 / h.max(1)) as u8;
        let pulse = triangle((frame as usize * 3 + y / 2) & 255) / 24;
        dst[y * w..(y + 1) * w].fill(color(18 + pulse, 6 + p / 16, 18 + p / 8));
    }
    fill_rect(dst, w, h, 0, h as isize / 2, w, 3, color(180, 40, 55));
}

fn draw_dialogue_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    draw_star_backdrop(dst, w, h, frame / 3);
    let bw = w.saturating_sub(w / 8);
    let bh = (h / 3).max(32);
    fill_rect(
        dst,
        w,
        h,
        (w - bw) as isize / 2,
        h as isize - bh as isize - 22,
        bw,
        bh,
        color(8, 16, 34),
    );
    fill_rect(
        dst,
        w,
        h,
        (w - bw) as isize / 2,
        h as isize - bh as isize - 22,
        bw,
        3,
        color(220, 220, 170),
    );
}

fn draw_dark_scanlines(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let scan = if (y + frame as usize / 4) & 7 == 0 {
            18
        } else {
            0
        };
        dst[y * w..(y + 1) * w].fill(color(2 + scan / 3, 6 + scan / 2, 18 + scan));
    }
}

fn draw_warning_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let pulse = triangle((frame as usize * 5) & 255) / 16;
    clear(dst, color(30 + pulse, 4, 8));
    for y in (0..h).step_by(32) {
        let c = if ((y / 32 + frame as usize / 10) & 1) == 0 {
            color(110, 20, 20)
        } else {
            color(20, 8, 12)
        };
        fill_rect(dst, w, h, 0, y as isize, w, 10, c);
    }
}

fn draw_signature_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(2, 12, 22));
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    for r in (24..w.min(h) / 2).step_by(28) {
        let c = tint_pixel(color(80, 160, 180), 80 + ((r + frame as usize) & 63) as u8);
        draw_ellipse(dst, w, h, cx, cy, r as isize, r as isize / 2, c);
    }
}

fn draw_comic_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(28, 30, 54));
    for i in 0..28usize {
        let x = (hash(i as u32 * 41) as usize + frame as usize) % w.max(1);
        fill_rect(dst, w, h, x as isize, 0, 5, h, color(42, 44, 72));
    }
}

fn draw_impact_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    let cell = 8usize;
    for y in (0..h).step_by(cell) {
        for x in (0..w).step_by(cell) {
            let d = ((x as isize - cx).unsigned_abs() + (y as isize - cy).unsigned_abs()) as u8;
            let pulse = triangle((d as usize + frame as usize * 4) & 255) / 5;
            fill_rect(
                dst,
                w,
                h,
                x as isize,
                y as isize,
                cell,
                cell,
                color(18 + pulse, 4, 8 + pulse / 2),
            );
        }
    }
}

fn draw_boot_grid(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(0, 12, 20));
    for y in (0..h).step_by(18) {
        let c = color(0, 42 + triangle((y + frame as usize) & 255) / 10, 52);
        fill_rect(dst, w, h, 0, y as isize, w, 1, c);
    }
    for x in (0..w).step_by(24) {
        fill_rect(dst, w, h, x as isize, 0, 1, h, color(0, 30, 42));
    }
}

fn draw_bubble_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let p = (y * 255 / h.max(1)) as u8;
        dst[y * w..(y + 1) * w].fill(color(0, 28 + p / 6, 62 + p / 5));
    }
    for i in 0..30usize {
        let x = hash(i as u32 * 13) as usize % w.max(1);
        let y = (hash(i as u32 * 33) as isize - frame as isize * (1 + i % 3) as isize)
            .rem_euclid(h.max(1) as isize) as usize;
        draw_ellipse(
            dst,
            w,
            h,
            x as isize,
            y as isize,
            5 + (i & 7) as isize,
            5 + (i & 7) as isize,
            color(80, 170, 210),
        );
    }
}

fn draw_intermission_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(4, 6, 18));
    let curtain = triangle((frame as usize * 2) & 255) as usize * w / 255;
    fill_rect(dst, w, h, 0, 0, curtain, h, color(65, 10, 30));
    fill_rect(
        dst,
        w,
        h,
        w.saturating_sub(curtain) as isize,
        0,
        curtain,
        h,
        color(65, 10, 30),
    );
}

fn draw_voice_backdrop(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(5, 8, 16));
    let center = h as isize / 2;
    for i in 0..18usize {
        let amp = triangle((frame as usize * 7 + i * 23) & 255) as isize / 4;
        let x = i * w / 18;
        fill_rect(
            dst,
            w,
            h,
            x as isize,
            center - amp,
            (w / 40).max(2),
            (amp * 2).max(2) as usize,
            color(45, 160, 220),
        );
    }
}

fn draw_dot_panel(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(6, 4, 8));
    for y in (8..h).step_by(10) {
        for x in (8..w).step_by(10) {
            let b = if ((x + y + frame as usize) / 10) & 7 == 0 {
                color(70, 36, 12)
            } else {
                color(22, 16, 10)
            };
            fill_rect(dst, w, h, x as isize, y as isize, 3, 3, b);
        }
    }
}

fn draw_rainbow_rasters(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        let idx = y / 8 + frame as usize / 3;
        dst[y * w..(y + 1) * w].fill(palette(idx, frame));
    }
}

fn draw_copper_split(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let split = (h as isize / 3 + wave(frame as usize * 2, h as isize / 8))
        .clamp(0, h.saturating_sub(1) as isize);
    for y in 0..h {
        let c = if y as isize <= split {
            color(10, 24 + (y * 80 / h.max(1)) as u8, 80)
        } else if y < h * 2 / 3 {
            color(54, 18, 70)
        } else {
            color(12, 50, 34)
        };
        dst[y * w..(y + 1) * w].fill(c);
    }
    fill_rect(dst, w, h, 0, split, w, 4, color(255, 220, 80));
}

fn draw_kefrens_bars(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    clear(dst, color(0, 0, 6));
    for y in 0..h {
        let x_center = w as isize / 2 + wave(y / 2 + frame as usize * 3, w as isize / 3);
        let width = 12 + triangle((y + frame as usize) & 255) as usize / 5;
        fill_rect(
            dst,
            w,
            h,
            x_center - width as isize / 2,
            y as isize,
            width,
            1,
            palette(y / 12, frame),
        );
    }
}

fn draw_moire_cells(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64, cell: usize) {
    let cell = cell.max(1);
    let cx1 = w as isize / 2 + wave(frame as usize * 2, w as isize / 5);
    let cy1 = h as isize / 2;
    let cx2 = w as isize / 2 - wave(frame as usize * 3, w as isize / 6);
    let cy2 = h as isize / 2 + wave(frame as usize * 2, h as isize / 6);
    for y in (0..h).step_by(cell) {
        for x in (0..w).step_by(cell) {
            let dx1 = (x as isize - cx1).unsigned_abs();
            let dy1 = (y as isize - cy1).unsigned_abs();
            let dx2 = (x as isize - cx2).unsigned_abs();
            let dy2 = (y as isize - cy2).unsigned_abs();
            let d1 = dx1 + dy1 / 2;
            let d2 = dx2 + dy2 / 2;
            let band = ((d1 + d2 + frame as usize) & 31) as u8;
            fill_rect(
                dst,
                w,
                h,
                x as isize,
                y as isize,
                cell,
                cell,
                color(band * 5, 20 + band * 3, 120 + band * 3),
            );
        }
    }
}

fn draw_plasma_cells(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64, cell: usize) {
    let cell = cell.max(1);
    for y in (0..h).step_by(cell) {
        for x in (0..w).step_by(cell) {
            fill_rect(
                dst,
                w,
                h,
                x as isize,
                y as isize,
                cell,
                cell,
                plasma_color(x as i32, y as i32, frame),
            );
        }
    }
}

fn draw_keftales(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    draw_moire_cells(dst, w, h, frame, 10);
    for y in (0..h).step_by(3) {
        let shift = wave(y + frame as usize * 2, w as isize / 6);
        fill_rect(
            dst,
            w,
            h,
            w as isize / 2 + shift,
            y as isize,
            28,
            2,
            palette(y / 8, frame),
        );
    }
}

fn draw_rotozoom_cells(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64) {
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    let zoom = 24 + triangle((frame as usize * 2) & 255) as isize / 4;
    for y in (0..h).step_by(6) {
        for x in (0..w).step_by(6) {
            let rx = x as isize - cx;
            let ry = y as isize - cy;
            let u = (rx + wave(frame as usize * 2, 64)) * zoom / 64 + ry / 3;
            let v = (ry - wave(frame as usize * 3, 64)) * zoom / 64 - rx / 3;
            let check = ((u / 24 + v / 24) & 1) == 0;
            fill_rect(
                dst,
                w,
                h,
                x as isize,
                y as isize,
                6,
                6,
                if check {
                    color(28, 110, 160)
                } else {
                    color(110, 40, 130)
                },
            );
        }
    }
}

fn draw_tunnel(dst: &mut [CameraPixel], w: usize, h: usize, frame: u64, images: &[CameraImage]) {
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    let maybe_img = images.first();
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let dx = x as isize - cx;
            let dy = y as isize - cy;
            let d = (dx.unsigned_abs() + dy.unsigned_abs()).max(1);
            let band = (d / 10 + frame as usize) & 15;
            let mut c = palette(band, frame);
            if let Some(img) = maybe_img {
                let sx = ((dx.unsigned_abs() + frame as usize) % img.w.max(1)).min(img.w - 1);
                let sy = ((dy.unsigned_abs() + frame as usize / 2) % img.h.max(1)).min(img.h - 1);
                c = blend(c, img.pixels[sy * img.stride + sx], 90);
            }
            fill_rect(dst, w, h, x as isize, y as isize, 4, 4, c);
        }
    }
}

fn render_insert_coin(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 170).max(2);
    let header_scale = scale.saturating_sub(1).max(1);
    draw_text_scaled(
        dst,
        w,
        h,
        "1UP   HIGH SCORE",
        18,
        20,
        header_scale,
        color(255, 40, 70),
        counters,
    );
    draw_text_scaled(
        dst,
        w,
        h,
        "00000  20000",
        18,
        20 + (header_scale * 10) as isize,
        header_scale,
        color(255, 245, 160),
        counters,
    );
    if (frame / 45) & 1 == 0 {
        let text = "INSERT COIN";
        let x = w as isize / 2 - text_width(text, scale) as isize / 2;
        draw_text_shadowed(
            dst,
            w,
            h,
            text,
            x,
            h as isize * 2 / 3,
            scale,
            color(255, 230, 80),
            counters,
        );
    } else {
        counters.hidden_glyph_count += "INSERTCOIN".len() as u64;
    }
}

fn render_initials_cursor(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 190).max(2);
    draw_text_scaled(
        dst,
        w,
        h,
        "RANK  SCORE  NAME",
        30,
        46,
        scale,
        color(255, 230, 120),
        counters,
    );
    for i in 0..5 {
        let y = 100 + i as isize * (scale as isize * 12);
        draw_text_scaled(
            dst,
            w,
            h,
            &format!(
                "{}    {:05}   {}",
                i + 1,
                50000 - i * 8200,
                ["AAA", "BCD", "EFG", "HIJ", "KLM"][i]
            ),
            42,
            y,
            scale,
            palette(i, frame),
            counters,
        );
    }
    let x = 42 + (scale * 6 * 13) as isize;
    let y = 100;
    if (frame / 40) & 1 == 0 {
        fill_rect(
            dst,
            w,
            h,
            x,
            y + (scale * 8) as isize,
            scale * 5,
            3,
            color(255, 255, 255),
        );
    }
}

fn render_sine_scroller(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 220).max(2);
    let long = text.repeat(6);
    let total = text_width(&long, scale).max(1);
    let offset = (frame as usize * scale * 2) % total;
    counters.scroll_offset = offset as u64;
    let mut x = w as isize - offset as isize;
    for (i, ch) in long.chars().enumerate() {
        let y = h as isize / 2 + wave(i * 16 + frame as usize * 4, h as isize / 5);
        let c = palette(i, frame);
        let pixels = draw_glyph_scaled(dst, w, h, ch, x, y, scale, c);
        if ch != ' ' {
            counters.record_glyph(pixels);
        }
        x += (scale * 6) as isize;
        if x > w as isize + 20 {
            break;
        }
    }
}

fn render_letter_bounce(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 180).max(2);
    let base_x = w as isize / 2 - text_width(text, scale) as isize / 2;
    for (i, ch) in text.chars().enumerate() {
        let y = h as isize / 2 + wave(frame as usize * 6 + i * 28, h as isize / 7);
        let c = palette(i, frame);
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            base_x + (i * scale * 6) as isize,
            y,
            scale,
            c,
        );
        if ch != ' ' {
            counters.record_glyph(pixels);
        }
    }
}

fn render_palette_chase(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 190).max(2);
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    for (i, ch) in text.chars().enumerate() {
        let c = palette(i, frame);
        counters.palette_step_count += 1;
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            x + (i * scale * 6) as isize,
            h as isize / 2,
            scale,
            c,
        );
        if ch != ' ' {
            counters.record_glyph(pixels);
        }
    }
}

fn render_zoom_from_horizon(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let text = "GO";
    let t = triangle((frame as usize * 3) & 255) as usize;
    let scale = 2 + t * (w / 90).max(4) / 255;
    let y = h as isize / 2 + (255 - t) as isize * h as isize / 640;
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    draw_text_shadowed(dst, w, h, text, x, y, scale, color(255, 245, 120), counters);
}

fn render_tile_snap(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let text = "MAGIK";
    let scale = (w / 250).max(2);
    let tile = scale * 8;
    let target_x = w as isize / 2 - (text.chars().count() * tile) as isize / 2;
    let target_y = h as isize / 2 - tile as isize / 2;
    let progress = ((frame % 90) as isize).min(60);
    for (i, ch) in text.chars().enumerate() {
        let sx = if i & 1 == 0 {
            -(tile as isize * 2)
        } else {
            w as isize + tile as isize
        };
        let sy = (hash(i as u32 * 93) as usize % h.max(1)) as isize;
        let tx = target_x + (i * tile) as isize;
        let x = sx + (tx - sx) * progress / 60;
        let y = sy + (target_y - sy) * progress / 60;
        fill_rect(dst, w, h, x - 2, y - 2, tile, tile, color(26, 38, 70));
        fill_rect(dst, w, h, x, y, tile - 4, tile - 4, palette(i, frame));
        counters.tile_count += 1;
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            x + scale as isize,
            y + scale as isize,
            scale,
            color(0, 0, 12),
        );
        counters.record_glyph(pixels);
    }
}

fn render_logo_shimmer(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 210).max(2);
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    draw_text_shadowed(
        dst,
        w,
        h,
        text,
        x,
        h as isize / 2 - (scale * 4) as isize,
        scale,
        color(80, 210, 255),
        counters,
    );
    let sweep = (frame as usize * 8) % (w + 160);
    fill_rect(
        dst,
        w,
        h,
        sweep as isize - 80,
        h as isize / 2 - 50,
        16,
        120,
        color(255, 255, 230),
    );
    counters.palette_step_count += 1;
}

fn render_rolling_digits(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 210).max(2);
    draw_text_scaled(
        dst,
        w,
        h,
        "SCORE",
        w as isize / 2 - text_width("SCORE", scale) as isize / 2,
        60,
        scale,
        color(255, 210, 70),
        counters,
    );
    let score = 123450 + frame * 37;
    let digits = format!("{:06}", score % 1_000_000);
    let x = w as isize / 2 - text_width(&digits, scale + 1) as isize / 2;
    for (i, ch) in digits.chars().enumerate() {
        let roll = wave(frame as usize * 5 + i * 31, scale as isize * 3);
        fill_rect(
            dst,
            w,
            h,
            x + (i * (scale + 1) * 6) as isize - 3,
            h as isize / 2 - 8,
            (scale + 1) * 6,
            (scale + 1) * 9,
            color(0, 0, 0),
        );
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            x + (i * (scale + 1) * 6) as isize,
            h as isize / 2 + roll,
            scale + 1,
            color(255, 245, 180),
        );
        counters.record_glyph(pixels);
    }
}

fn render_ready_go(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let text = if (frame / 50) & 1 == 0 { "READY" } else { "GO" };
    let slap = 255 - triangle((frame as usize * 5) & 255) as usize;
    let scale = (w / 220).max(2) + slap / 90;
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    draw_text_shadowed(
        dst,
        w,
        h,
        text,
        x,
        h as isize / 2 - (scale * 4) as isize,
        scale,
        color(255, 235, 90),
        counters,
    );
    for i in 0..36usize {
        let x = w as isize / 2
            + ((hash(i as u32 * 11) & 127) as isize - 63) * (frame % 40) as isize / 10;
        let y = h as isize / 2
            + ((hash(i as u32 * 17) & 127) as isize - 63) * (frame % 40) as isize / 10;
        fill_rect(dst, w, h, x, y, 4, 4, palette(i, frame));
        counters.bob_count += 1;
    }
}

fn render_typewriter(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 260).max(2);
    let visible = ((frame / 5) as usize % (text.len() + 6)).min(text.len());
    let shown = &text[..visible];
    draw_text_scaled(
        dst,
        w,
        h,
        shown,
        w as isize / 8,
        h as isize * 2 / 3,
        scale,
        color(230, 245, 210),
        counters,
    );
    if (frame / 20) & 1 == 0 {
        fill_rect(
            dst,
            w,
            h,
            w as isize / 8 + text_width(shown, scale) as isize,
            h as isize * 2 / 3,
            scale * 4,
            scale * 7,
            color(255, 245, 180),
        );
    }
}

fn render_vector_draw(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 160).max(3);
    let total = text.chars().count() * 12;
    let limit = (frame as usize / 2) % (total + 16);
    draw_vector_word(
        dst,
        w,
        h,
        text,
        w as isize / 2,
        h as isize / 2,
        scale as isize * 5,
        0.0,
        limit,
        color(100, 255, 190),
        counters,
    );
}

fn draw_vector_word(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    text: &str,
    cx: isize,
    cy: isize,
    size: isize,
    angle: f32,
    limit: usize,
    c: CameraPixel,
    counters: &mut TextCounters,
) {
    let count = text.chars().count().max(1);
    let width = count as isize * size / 2;
    let mut seg_seen = 0usize;
    for (i, ch) in text.chars().enumerate() {
        let ox = i as isize * size / 2 - width / 2;
        let segments = vector_segments(ch);
        for &(x0, y0, x1, y1) in segments {
            if seg_seen >= limit {
                return;
            }
            let p0 = rotate_point(ox + x0 * size / 10, y0 * size / 10 - size / 2, angle);
            let p1 = rotate_point(ox + x1 * size / 10, y1 * size / 10 - size / 2, angle);
            draw_line(dst, w, h, cx + p0.0, cy + p0.1, cx + p1.0, cy + p1.1, c);
            counters.vector_segment_count += 1;
            seg_seen += 1;
        }
        counters.glyph_count += 1;
        counters.glyph_pixels += 1;
    }
}

fn rotate_point(x: isize, y: isize, angle: f32) -> (isize, isize) {
    let s = angle.sin();
    let c = angle.cos();
    (
        (x as f32 * c - y as f32 * s) as isize,
        (x as f32 * s + y as f32 * c) as isize,
    )
}

type Segment = (isize, isize, isize, isize);

fn vector_segments(ch: char) -> &'static [Segment] {
    match ch.to_ascii_uppercase() {
        'A' => &[(0, 10, 5, 0), (5, 0, 10, 10), (2, 5, 8, 5)],
        'B' => &[
            (0, 0, 0, 10),
            (0, 0, 7, 2),
            (7, 2, 0, 5),
            (0, 5, 7, 8),
            (7, 8, 0, 10),
        ],
        'C' => &[(9, 1, 1, 1), (1, 1, 1, 9), (1, 9, 9, 9)],
        'D' => &[(0, 0, 0, 10), (0, 0, 8, 3), (8, 3, 8, 7), (8, 7, 0, 10)],
        'E' => &[(9, 0, 0, 0), (0, 0, 0, 10), (0, 5, 7, 5), (0, 10, 9, 10)],
        'F' => &[(0, 0, 0, 10), (0, 0, 9, 0), (0, 5, 7, 5)],
        'G' => &[
            (9, 1, 1, 1),
            (1, 1, 1, 9),
            (1, 9, 9, 9),
            (9, 9, 9, 5),
            (9, 5, 5, 5),
        ],
        'H' => &[(0, 0, 0, 10), (10, 0, 10, 10), (0, 5, 10, 5)],
        'I' => &[(1, 0, 9, 0), (5, 0, 5, 10), (1, 10, 9, 10)],
        'K' => &[(0, 0, 0, 10), (10, 0, 0, 5), (0, 5, 10, 10)],
        'L' => &[(0, 0, 0, 10), (0, 10, 9, 10)],
        'M' => &[(0, 10, 0, 0), (0, 0, 5, 5), (5, 5, 10, 0), (10, 0, 10, 10)],
        'N' => &[(0, 10, 0, 0), (0, 0, 10, 10), (10, 10, 10, 0)],
        'O' => &[(1, 1, 9, 1), (9, 1, 9, 9), (9, 9, 1, 9), (1, 9, 1, 1)],
        'P' => &[(0, 10, 0, 0), (0, 0, 9, 1), (9, 1, 9, 5), (9, 5, 0, 5)],
        'R' => &[
            (0, 10, 0, 0),
            (0, 0, 9, 1),
            (9, 1, 9, 5),
            (9, 5, 0, 5),
            (0, 5, 10, 10),
        ],
        'S' => &[
            (9, 1, 1, 1),
            (1, 1, 1, 5),
            (1, 5, 9, 5),
            (9, 5, 9, 9),
            (9, 9, 1, 9),
        ],
        'T' => &[(0, 0, 10, 0), (5, 0, 5, 10)],
        'U' => &[(0, 0, 0, 9), (0, 9, 10, 9), (10, 9, 10, 0)],
        'V' => &[(0, 0, 5, 10), (5, 10, 10, 0)],
        'X' => &[(0, 0, 10, 10), (10, 0, 0, 10)],
        'Y' => &[(0, 0, 5, 5), (10, 0, 5, 5), (5, 5, 5, 10)],
        'Z' => &[(0, 0, 10, 0), (10, 0, 0, 10), (0, 10, 10, 10)],
        _ => &[
            (0, 0, 10, 0),
            (10, 0, 10, 10),
            (10, 10, 0, 10),
            (0, 10, 0, 0),
        ],
    }
}

fn render_continue_countdown(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 190).max(2);
    let n = 9 - ((frame / 50) % 10);
    draw_text_shadowed(
        dst,
        w,
        h,
        "CONTINUE?",
        w as isize / 2 - text_width("CONTINUE?", scale) as isize / 2,
        h as isize / 3,
        scale,
        color(255, 255, 220),
        counters,
    );
    let digit = n.to_string();
    draw_text_shadowed(
        dst,
        w,
        h,
        &digit,
        w as isize / 2 - text_width(&digit, scale + 3) as isize / 2,
        h as isize / 2,
        scale + 3,
        palette(frame as usize / 5, frame),
        counters,
    );
}

fn render_trackball_signature(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 260).max(2);
    let initials = ['A', 'C', 'E'];
    let cx = w as isize / 2;
    let cy = h as isize / 2;
    for (i, ch) in initials.iter().enumerate() {
        let x = cx + wave(frame as usize * 3 + i * 64, w as isize / 5);
        let y = cy + wave(frame as usize * 4 + i * 77, h as isize / 5);
        draw_ellipse(
            dst,
            w,
            h,
            x + (scale * 3) as isize,
            y + (scale * 3) as isize,
            (scale * 5) as isize,
            (scale * 5) as isize,
            color(30, 80, 90),
        );
        let pixels = draw_glyph_scaled(dst, w, h, *ch, x, y, scale, palette(i, frame));
        counters.record_glyph(pixels);
    }
    for i in 0..42usize {
        let x = cx + wave(frame as usize * 3 + i * 9, w as isize / 4);
        let y = cy + wave(frame as usize * 4 + i * 11, h as isize / 4);
        put(dst, w, h, x, y, color(180, 255, 210));
        counters.bob_count += 1;
    }
}

fn render_grawlix(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 220).max(2);
    let bw = text_width("#$%&!*", scale) + 40;
    let bh = scale * 12;
    let x = w as isize / 2 - bw as isize / 2 + wave(frame as usize * 2, 18);
    let y = h as isize / 2 - bh as isize / 2;
    fill_rect(dst, w, h, x, y, bw, bh, color(242, 242, 210));
    fill_rect(dst, w, h, x + 4, y + 4, bw - 8, bh - 8, color(20, 25, 40));
    draw_text_scaled(
        dst,
        w,
        h,
        "#$%&!*",
        x + 18,
        y + scale as isize * 2,
        scale,
        palette(frame as usize / 4, frame),
        counters,
    );
}

fn render_center_title(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    scale: usize,
    counters: &mut TextCounters,
) {
    let scale = scale.max((w / 260).max(2));
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    let y = h as isize / 2 - (scale * 4) as isize + wave(frame as usize * 2, 12);
    draw_text_shadowed(dst, w, h, text, x, y, scale, color(255, 250, 180), counters);
}

fn render_text_fill(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    mode: FillMode,
    counters: &mut TextCounters,
) {
    let scale = (w / 150).max(3);
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    let y = h as isize / 2 - (scale * 4) as isize;
    draw_text_palette_rows(dst, w, h, text, x, y, scale, frame, mode, counters);
}

fn render_quote_box(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 300).max(2);
    let bw = w * 3 / 4;
    fill_rect(
        dst,
        w,
        h,
        (w - bw) as isize / 2,
        h as isize / 2 - 55,
        bw,
        110,
        color(8, 12, 24),
    );
    fill_rect(
        dst,
        w,
        h,
        (w - bw) as isize / 2,
        h as isize / 2 - 55,
        bw,
        4,
        palette(frame as usize / 4, frame),
    );
    draw_text_scaled(
        dst,
        w,
        h,
        "YOU WIN",
        w as isize / 2 - text_width("YOU WIN", scale + 1) as isize / 2,
        h as isize / 2 - 36,
        scale + 1,
        color(255, 235, 120),
        counters,
    );
    draw_text_scaled(
        dst,
        w,
        h,
        "\"SKILL BEATS LUCK\"",
        w as isize / 2 - text_width("\"SKILL BEATS LUCK\"", scale) as isize / 2,
        h as isize / 2 + 18,
        scale,
        color(220, 245, 255),
        counters,
    );
}

fn render_tip_ticker(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 320).max(2);
    draw_text_shadowed(
        dst,
        w,
        h,
        "CONTINUE?",
        w as isize / 2 - text_width("CONTINUE?", scale + 1) as isize / 2,
        h as isize / 3,
        scale + 1,
        color(255, 230, 90),
        counters,
    );
    render_sine_scroller(
        dst,
        w,
        h,
        frame,
        "TIP: HOLD FIRE FOR A CHARGED SHOT  ",
        counters,
    );
}

fn render_finish_prompt(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 155).max(3);
    let shake = wave(frame as usize * 12, 16);
    draw_text_shadowed(
        dst,
        w,
        h,
        "FINISH HIM",
        w as isize / 2 - text_width("FINISH HIM", scale) as isize / 2 + shake,
        h as isize / 2 - (scale * 4) as isize,
        scale,
        color(255, 40, 32),
        counters,
    );
}

fn render_boot_slogan(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 240).max(2);
    let visible = (frame / 30) & 1 == 0;
    draw_text_scaled(
        dst,
        w,
        h,
        "MISTER MAGIK",
        w as isize / 2 - text_width("MISTER MAGIK", scale) as isize / 2,
        h as isize / 3,
        scale,
        color(255, 245, 180),
        counters,
    );
    if visible {
        draw_text_scaled(
            dst,
            w,
            h,
            "100 MEGA SHOCK",
            w as isize / 2 - text_width("100 MEGA SHOCK", scale) as isize / 2,
            h as isize * 2 / 3,
            scale,
            color(80, 245, 255),
            counters,
        );
    } else {
        counters.hidden_glyph_count += 12;
    }
}

fn render_letter_bubbles(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 300).max(2);
    for (i, ch) in "EXTEND".chars().enumerate() {
        let x = w as isize / 2 - 6 * (scale * 8) as isize / 2 + i as isize * (scale * 8) as isize;
        let y = h as isize / 2 + wave(frame as usize * 3 + i * 41, h as isize / 5);
        draw_ellipse(
            dst,
            w,
            h,
            x + (scale * 3) as isize,
            y + (scale * 4) as isize,
            (scale * 5) as isize,
            (scale * 5) as isize,
            color(90, 190, 230),
        );
        let pixels = draw_glyph_scaled(dst, w, h, ch, x, y, scale, color(255, 255, 255));
        counters.record_glyph(pixels);
        counters.bob_count += 1;
    }
}

fn render_phrase_meter(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 300).max(2);
    let phrase = "BONUS";
    let count = ((frame / 28) as usize % (phrase.len() + 1)).max(1);
    for (i, ch) in phrase.chars().enumerate() {
        let x = w as isize / 2 - text_width(phrase, scale + 1) as isize / 2
            + (i * (scale + 1) * 6) as isize;
        let c = if i < count {
            palette(i, frame)
        } else {
            color(45, 45, 58)
        };
        let pixels = draw_glyph_scaled(dst, w, h, ch, x, h as isize / 2, scale + 1, c);
        counters.record_glyph(pixels);
    }
    fill_rect(
        dst,
        w,
        h,
        w as isize / 4,
        h as isize * 2 / 3,
        w / 2 * count / phrase.len(),
        10,
        color(255, 230, 80),
    );
}

fn render_powerup_letters(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 320).max(2);
    for (i, ch) in "SLFB".chars().enumerate() {
        let pop = triangle((frame as usize * 5 + i * 48) & 255) as usize;
        let size = scale + pop / 128;
        let x = w as isize / 2 - 2 * (scale * 14) as isize + i as isize * (scale * 14) as isize;
        let y = h as isize / 2 + wave(frame as usize * 2 + i * 30, 28);
        fill_rect(
            dst,
            w,
            h,
            x - 6,
            y - 6,
            size * 9,
            size * 9,
            color(8, 10, 20),
        );
        let pixels = draw_glyph_scaled(dst, w, h, ch, x, y, size, palette(i, frame));
        counters.record_glyph(pixels);
        counters.tile_count += 1;
    }
}

fn render_intermission_card(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 300).max(2);
    let card_w = w * 2 / 3;
    let card_h = h / 3;
    let x = (w - card_w) as isize / 2;
    let y = h as isize / 2 - card_h as isize / 2 + wave(frame as usize * 2, 18);
    fill_rect(dst, w, h, x, y, card_w, card_h, color(232, 222, 176));
    fill_rect(
        dst,
        w,
        h,
        x + 6,
        y + 6,
        card_w.saturating_sub(12),
        card_h.saturating_sub(12),
        color(12, 20, 42),
    );
    draw_text_scaled(
        dst,
        w,
        h,
        "ACT 2",
        w as isize / 2 - text_width("ACT 2", scale + 1) as isize / 2,
        y + 26,
        scale + 1,
        color(255, 220, 90),
        counters,
    );
    draw_text_scaled(
        dst,
        w,
        h,
        "THE CITY WAKES",
        w as isize / 2 - text_width("THE CITY WAKES", scale) as isize / 2,
        y + card_h as isize - 48,
        scale,
        color(220, 240, 255),
        counters,
    );
}

fn render_attract_pages(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 320).max(2);
    let page = (frame / 120) & 2;
    let lines: &[&str] = if page == 0 {
        &["HOW TO PLAY", "MOVE  FIRE", "RESCUE ALL HUMANS"]
    } else {
        &["BONUS ITEMS", "COLLECT LETTERS", "SPELL MAGIK"]
    };
    for (i, line) in lines.iter().enumerate() {
        draw_text_scaled(
            dst,
            w,
            h,
            line,
            w as isize / 2 - text_width(line, scale) as isize / 2,
            h as isize / 3 + i as isize * (scale * 12) as isize,
            scale,
            palette(i, frame),
            counters,
        );
    }
}

fn render_wave_banner(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 230).max(2);
    let wave_no = 1 + (frame / 90) % 99;
    let text = format!("WAVE {}", wave_no);
    let x = (w as isize - (frame as isize * 6 % (w as isize + text_width(&text, scale) as isize)))
        + wave(frame as usize * 4, 12);
    fill_rect(dst, w, h, 0, h as isize / 2 - 34, w, 68, color(8, 20, 34));
    draw_text_shadowed(
        dst,
        w,
        h,
        &text,
        x,
        h as isize / 2 - (scale * 4) as isize,
        scale,
        color(255, 235, 90),
        counters,
    );
    counters.scroll_offset = (frame * 6) % w.max(1) as u64;
}

fn render_voice_sync(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 190).max(2);
    let pulse = triangle((frame as usize * 8) & 255) as usize / 96;
    draw_text_shadowed(
        dst,
        w,
        h,
        "GET READY",
        w as isize / 2 - text_width("GET READY", scale + pulse) as isize / 2,
        h as isize / 2 - 35,
        scale + pulse,
        color(120, 255, 230),
        counters,
    );
}

fn render_dot_matrix_roll(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 360).max(2);
    let lines = [
        "CREDITS",
        "CODE  NIGEL",
        "PIXELS  MAGIK",
        "THANK YOU",
        "2026",
    ];
    for (i, line) in lines.iter().enumerate() {
        let y = h as isize + i as isize * (scale * 12) as isize
            - (frame as isize * 2 % (h as isize + 160));
        draw_text_scaled(
            dst,
            w,
            h,
            line,
            w as isize / 2 - text_width(line, scale) as isize / 2,
            y,
            scale,
            color(255, 160, 60),
            counters,
        );
        counters.tile_count += line.len() as u64;
    }
    counters.scroll_offset = (frame * 2) % h.max(1) as u64;
}

fn render_copper_credits(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 330).max(2);
    let lines = [
        "CODE  COPPER",
        "BLITTER  BOBS",
        "MUSIC  MOD",
        "GFX  RASTERS",
    ];
    for (i, line) in lines.iter().enumerate() {
        let y = (h as isize / 5
            + i as isize * (scale * 13) as isize
            + wave(frame as usize * 2 + i * 45, 16))
        .rem_euclid(h.max(1) as isize);
        draw_text_scaled(dst, w, h, line, 30, y, scale, palette(i, frame), counters);
    }
}

fn render_bob_swarm(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 420).max(1);
    let chars: Vec<char> = "AMIGADEMOSCENE".chars().collect();
    for i in 0..96usize {
        let x = (hash(i as u32 * 31) as usize + frame as usize * (1 + i % 4)) % (w + 50).max(1);
        let y = (hash(i as u32 * 67) as isize + wave(frame as usize * 2 + i * 13, h as isize / 3))
            .rem_euclid(h.max(1) as isize) as usize;
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            chars[i % chars.len()],
            x as isize - 25,
            y as isize,
            scale + 1,
            palette(i, frame),
        );
        counters.record_glyph(pixels);
        counters.bob_count += 1;
    }
}

fn render_bob_path_text(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 360).max(2);
    let text: Vec<char> = "BOB PATH SCROLLTEXT  ".chars().collect();
    for i in 0..42usize {
        let phase = frame as usize * 3 + i * 13;
        let x = w as isize / 2 + wave(phase, w as isize / 3);
        let y = h as isize / 2 + wave(phase * 2 + 90, h as isize / 4);
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            text[i % text.len()],
            x,
            y,
            scale,
            palette(i, frame),
        );
        if text[i % text.len()] != ' ' {
            counters.record_glyph(pixels);
        }
        counters.bob_count += 1;
    }
}

fn render_shadebob_writing(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let text = "SHADEBOB";
    let scale = (w / 240).max(2);
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    let y = h as isize / 2 - 20;
    for i in 0..80usize {
        let bx = x + wave(
            frame as usize * 4 + i * 9,
            text_width(text, scale) as isize / 2,
        );
        let by = y + wave(frame as usize * 3 + i * 11, 50);
        draw_ellipse(dst, w, h, bx, by, 18, 10, tint_pixel(palette(i, frame), 95));
        counters.bob_count += 1;
    }
    let reveal = ((frame / 6) as usize % (text.len() + 1)).max(1);
    draw_text_shadowed(
        dst,
        w,
        h,
        &text[..reveal],
        x,
        y,
        scale,
        color(255, 255, 230),
        counters,
    );
}

fn render_infinite_bob_trail(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 400).max(1) + 1;
    let text: Vec<char> = "INFINITE BOBS ".chars().collect();
    for i in 0..180usize {
        let x = (w as isize / 2 + wave(frame as usize * 2 + i * 7, w as isize / 2))
            .rem_euclid(w.max(1) as isize);
        let y = (h as isize / 2 + wave(frame as usize * 3 + i * 5, h as isize / 2))
            .rem_euclid(h.max(1) as isize);
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            text[i % text.len()],
            x,
            y,
            scale,
            tint_pixel(palette(i, frame), 90 + (i & 63) as u8),
        );
        if text[i % text.len()] != ' ' {
            counters.record_glyph(pixels);
        }
        counters.bob_count += 1;
    }
}

fn render_kefrens_text(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 240).max(2);
    let text = "KEFRENS";
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    let wipe = (frame as usize * 8) % (w + 1);
    draw_text_shadowed(
        dst,
        w,
        h,
        text,
        x,
        h as isize / 2 - 20,
        scale,
        color(255, 250, 180),
        counters,
    );
    fill_rect(
        dst,
        w,
        h,
        wipe as isize,
        0,
        w.saturating_sub(wipe),
        h,
        color(0, 0, 6),
    );
    counters.palette_step_count += h as u64;
}

fn render_plasma_scrolltext(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 250).max(2);
    let text = "PLASMA SCROLLTEXT  ".repeat(4);
    let total = text_width(&text, scale).max(1);
    let offset = (frame as usize * 3) % total;
    counters.scroll_offset = offset as u64;
    let mut x = w as isize - offset as isize;
    for (i, ch) in text.chars().enumerate() {
        let c = plasma_color(x as i32, h as i32 / 2 + i as i32, frame);
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            x,
            h as isize / 2 + wave(i * 12 + frame as usize * 2, 26),
            scale,
            c,
        );
        if ch != ' ' {
            counters.record_glyph(pixels);
        }
        counters.palette_step_count += 1;
        x += (scale * 6) as isize;
        if x > w as isize + 20 {
            break;
        }
    }
}

fn render_roto_text(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let angle = frame as f32 * 0.035;
    draw_vector_word(
        dst,
        w,
        h,
        "ROTO",
        w as isize / 2,
        h as isize / 2,
        (w.min(h) / 3).max(30) as isize,
        angle,
        100,
        color(255, 245, 180),
        counters,
    );
}

fn render_wobbler_text(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 260).max(2);
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    for (i, ch) in text.chars().enumerate() {
        let y = h as isize / 2 + wave(frame as usize * 4 + i * 18, 50);
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            x + (i * scale * 6) as isize,
            y,
            scale,
            palette(i, frame),
        );
        if ch != ' ' {
            counters.record_glyph(pixels);
        }
    }
}

fn render_tunnel_ribbon(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let scale = (w / 340).max(2);
    let text: Vec<char> = "TUNNEL RIBBON ".chars().collect();
    for i in 0..36usize {
        let z = 20 + ((i * 11 + frame as usize * 2) & 255);
        let x =
            w as isize / 2 + wave(i * 18 + frame as usize * 3, w as isize / 3) * z as isize / 220;
        let y =
            h as isize / 2 + wave(i * 22 + frame as usize * 2, h as isize / 3) * z as isize / 220;
        let s = scale + 255usize.saturating_sub(z) / 128;
        let pixels = draw_glyph_scaled(dst, w, h, text[i % text.len()], x, y, s, palette(i, frame));
        if text[i % text.len()] != ' ' {
            counters.record_glyph(pixels);
        }
        counters.bob_count += 1;
    }
}

fn render_vector_spin(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let angle = frame as f32 * 0.06;
    draw_vector_word(
        dst,
        w,
        h,
        text,
        w as isize / 2,
        h as isize / 2,
        (w.min(h) / 3).max(30) as isize,
        angle,
        100,
        color(140, 255, 255),
        counters,
    );
}

fn render_turntable_logo(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let text = "MAGIK";
    let scale = (w / 190).max(2);
    let phase = triangle((frame as usize * 3) & 255) as usize;
    let squash = (30 + phase) as isize;
    let x0 = w as isize / 2 - text_width(text, scale) as isize * squash / 255 / 2;
    for (i, ch) in text.chars().enumerate() {
        let x = x0 + (i * scale * 6) as isize * squash / 255;
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            x,
            h as isize / 2 - 25,
            scale,
            palette(i, frame),
        );
        counters.record_glyph(pixels);
    }
}

fn render_layered_transparent_text(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 170).max(3);
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    let y = h as isize / 2 - 34;
    for layer in 0..4usize {
        draw_text_scaled(
            dst,
            w,
            h,
            text,
            x + wave(frame as usize * 2 + layer * 40, 28),
            y + layer as isize * 9,
            scale,
            tint_pixel(palette(layer, frame), 120),
            counters,
        );
    }
}

fn render_rubber_text(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    text: &str,
    counters: &mut TextCounters,
) {
    let scale = (w / 180).max(3);
    let x = w as isize / 2 - text_width(text, scale) as isize / 2;
    for (i, ch) in text.chars().enumerate() {
        let s = scale + (triangle((frame as usize * 5 + i * 37) & 255) as usize / 96);
        let y = h as isize / 2 - (s * 4) as isize + wave(frame as usize * 4 + i * 20, 34);
        let pixels = draw_glyph_scaled(
            dst,
            w,
            h,
            ch,
            x + (i * scale * 6) as isize,
            y,
            s,
            palette(i, frame),
        );
        counters.record_glyph(pixels);
    }
}

fn render_scroll_explode(
    dst: &mut [CameraPixel],
    w: usize,
    h: usize,
    frame: u64,
    counters: &mut TextCounters,
) {
    let text: Vec<char> = "SCROLLTEXT EXPLODE ".chars().collect();
    let scale = (w / 330).max(2);
    let phase = (frame % 150) as isize;
    for i in 0..48usize {
        let base_x = w as isize
            - ((frame as isize * 4 + i as isize * (scale * 6) as isize) % (w as isize + 240));
        let base_y = h as isize / 2 + wave(i * 13, 38);
        let burst = if phase < 75 { phase } else { 150 - phase };
        let x = base_x + ((hash(i as u32 * 19) & 63) as isize - 31) * burst / 18;
        let y = base_y + ((hash(i as u32 * 23) & 63) as isize - 31) * burst / 18;
        let ch = text[i % text.len()];
        let pixels = draw_glyph_scaled(dst, w, h, ch, x, y, scale, palette(i, frame));
        if ch != ' ' {
            counters.record_glyph(pixels);
        }
        counters.bob_count += 1;
    }
    counters.scroll_offset = (frame * 4) % w.max(1) as u64;
}

fn draw_label(dst: &mut [CameraPixel], w: usize, h: usize, text: &str) {
    let bg_h = 18usize.min(h);
    fill_rect(dst, w, h, 0, 0, w, bg_h, color(0, 0, 0));
    let fg = color(255, 245, 170);
    let mut x0 = 6usize;
    for ch in text.chars().take(120) {
        if x0 + 6 >= w {
            break;
        }
        let glyph = glyph5x7(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    put(dst, w, h, (x0 + col) as isize, (5 + row) as isize, fg);
                }
            }
        }
        x0 += 6;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_frame(pixels: &[CameraPixel]) -> u64 {
        pixels.iter().fold(0xcbf2_9ce4_8422_2325, |acc, px| {
            (acc ^ px.0 as u64).wrapping_mul(0x1000_0000_01b3)
        })
    }

    #[test]
    fn labels_parse_in_stable_order() {
        let labels = TextEffectKind::labels();
        assert!(labels.contains("insert-coin-blink-cadence"));
        assert!(labels.contains("amiga-scrolltext-explode-reassemble"));
        assert_eq!(TextEffectKind::all().len(), 50);
        assert_eq!(
            TextEffectKind::all()[0].label(),
            "insert-coin-blink-cadence"
        );
        assert_eq!(
            TextEffectKind::all()[49].label(),
            "amiga-scrolltext-explode-reassemble"
        );
        for kind in TextEffectKind::all() {
            assert_eq!(TextEffectKind::parse(kind.label()), Some(*kind));
            assert_eq!(
                TextEffectKind::parse(&kind.label().replace('-', "_")),
                Some(*kind)
            );
        }
        assert!(TextEffectKind::parse("bogus").is_none());
    }

    #[test]
    fn renders_every_effect_deterministically_and_nonblank() {
        let w = 96;
        let h = 54;
        let images = synthetic_text_images(4);
        for &kind in TextEffectKind::all() {
            let mut state_a = TextEffectRenderState::new(w, h);
            let mut state_b = TextEffectRenderState::new(w, h);
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            render_text_effect_frame(&mut a, &mut state_a, w, h, &images, kind, 7, None);
            render_text_effect_frame(&mut b, &mut state_b, w, h, &images, kind, 7, None);
            assert_eq!(a, b, "{kind:?} should be deterministic");
            assert!(a.iter().any(|px| px.0 != 0), "{kind:?} should draw pixels");
        }
    }

    #[test]
    fn animated_effects_change_between_frames() {
        let w = 96;
        let h = 54;
        let images = synthetic_text_images(4);
        for &kind in TextEffectKind::all() {
            let mut state = TextEffectRenderState::new(w, h);
            let mut a = vec![CameraPixel(0); w * h];
            let mut b = vec![CameraPixel(0); w * h];
            render_text_effect_frame(&mut a, &mut state, w, h, &images, kind, 0, None);
            render_text_effect_frame(&mut b, &mut state, w, h, &images, kind, 60, None);
            assert_ne!(
                hash_frame(&a),
                hash_frame(&b),
                "{kind:?} should visibly animate"
            );
        }
    }

    #[test]
    fn stats_draw_sum_matches_buckets() {
        let w = 64;
        let h = 36;
        let images = synthetic_text_images(2);
        let mut state = TextEffectRenderState::new(w, h);
        let mut frame = vec![CameraPixel(0); w * h];
        let stats = render_text_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            TextEffectKind::AmigaVectorLineFontSpin,
            12,
            Some("amiga-vector-line-font-spin"),
        );
        assert_eq!(
            stats.draw_us(),
            stats.clear_us
                + stats.background_us
                + stats.projection_us
                + stats.image_blit_us
                + stats.sprite_us
                + stats.post_us
                + stats.hud_us
        );
    }

    #[test]
    fn text_counters_cover_specialized_effects() {
        let w = 96;
        let h = 54;
        let images = synthetic_text_images(1);
        let mut state = TextEffectRenderState::new(w, h);
        let mut frame = vec![CameraPixel(0); w * h];
        let vector = render_text_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            TextEffectKind::AmigaVectorLineFontSpin,
            20,
            None,
        );
        assert!(vector.vector_segment_count > 0);
        assert!(vector.glyph_count > 0);

        let bobs = render_text_effect_frame(
            &mut frame,
            &mut state,
            w,
            h,
            &images,
            TextEffectKind::AmigaBlitterBobLetterSwarm,
            20,
            None,
        );
        assert!(bobs.bob_count > 0);
        assert!(bobs.glyph_count > 0);
    }

    #[test]
    fn small_sizes_do_not_panic() {
        let images = synthetic_text_images(1);
        for &kind in TextEffectKind::all() {
            let w = 8;
            let h = 6;
            let mut state = TextEffectRenderState::new(w, h);
            let mut frame = vec![CameraPixel(0); w * h];
            render_text_effect_frame(&mut frame, &mut state, w, h, &images, kind, 3, None);
        }
    }
}
