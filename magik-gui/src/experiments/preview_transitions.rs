//! Experiment-only screenshot preview transition catalog.

use crate::screenshot_transitions::PreviewTransitionEffect;

pub(crate) const MEGA: [PreviewTransitionEffect; 33] = [
    PreviewTransitionEffect::Fade,
    PreviewTransitionEffect::Wipe,
    PreviewTransitionEffect::Slide,
    PreviewTransitionEffect::Zoom,
    PreviewTransitionEffect::Scanline,
    PreviewTransitionEffect::Checker,
    PreviewTransitionEffect::Dissolve,
    PreviewTransitionEffect::CrtBeamWipe,
    PreviewTransitionEffect::MosaicResolve,
    PreviewTransitionEffect::CopperBars,
    PreviewTransitionEffect::VenetianBlinds,
    PreviewTransitionEffect::BarnDoor,
    PreviewTransitionEffect::Iris,
    PreviewTransitionEffect::ClockWipe,
    PreviewTransitionEffect::SpriteStrips,
    PreviewTransitionEffect::StarfieldWarp,
    PreviewTransitionEffect::VectorRedraw,
    PreviewTransitionEffect::PaletteCycle,
    PreviewTransitionEffect::RasterTear,
    PreviewTransitionEffect::TileLoader,
    PreviewTransitionEffect::VenetianCopper,
    PreviewTransitionEffect::AttributeFlash,
    PreviewTransitionEffect::TecTec,
    PreviewTransitionEffect::Linecrunch,
    PreviewTransitionEffect::RacingBeam,
    PreviewTransitionEffect::SpriteMultiplex,
    PreviewTransitionEffect::RowScrollParallax,
    PreviewTransitionEffect::SuperScalerPop,
    PreviewTransitionEffect::MaskBlit,
    PreviewTransitionEffect::PhosphorDecay,
    PreviewTransitionEffect::PlasmaMask,
    PreviewTransitionEffect::MoireRings,
    PreviewTransitionEffect::KefrensCurtain,
];

pub(crate) fn all() -> &'static [PreviewTransitionEffect] {
    &MEGA
}

pub(crate) fn label(effect: PreviewTransitionEffect) -> &'static str {
    match effect {
        PreviewTransitionEffect::Fade => "fade",
        PreviewTransitionEffect::Wipe => "wipe",
        PreviewTransitionEffect::Slide => "slide",
        PreviewTransitionEffect::Zoom => "zoom",
        PreviewTransitionEffect::Scanline => "scanline",
        PreviewTransitionEffect::Checker => "checker",
        PreviewTransitionEffect::Dissolve => "dissolve",
        PreviewTransitionEffect::CrtBeamWipe => "crt-beam-wipe",
        PreviewTransitionEffect::MosaicResolve => "mosaic-resolve",
        PreviewTransitionEffect::CopperBars => "copper-bars",
        PreviewTransitionEffect::VenetianBlinds => "venetian-blinds",
        PreviewTransitionEffect::BarnDoor => "barn-door",
        PreviewTransitionEffect::Iris => "iris",
        PreviewTransitionEffect::ClockWipe => "clock-wipe",
        PreviewTransitionEffect::SpriteStrips => "sprite-strips",
        PreviewTransitionEffect::StarfieldWarp => "starfield-warp",
        PreviewTransitionEffect::VectorRedraw => "vector-redraw",
        PreviewTransitionEffect::PaletteCycle => "palette-cycle",
        PreviewTransitionEffect::RasterTear => "raster-tear",
        PreviewTransitionEffect::TileLoader => "tile-loader",
        PreviewTransitionEffect::VenetianCopper => "venetian-copper",
        PreviewTransitionEffect::AttributeFlash => "attribute-flash",
        PreviewTransitionEffect::TecTec => "tec-tec",
        PreviewTransitionEffect::Linecrunch => "linecrunch",
        PreviewTransitionEffect::RacingBeam => "racing-beam",
        PreviewTransitionEffect::SpriteMultiplex => "sprite-multiplex",
        PreviewTransitionEffect::RowScrollParallax => "row-scroll-parallax",
        PreviewTransitionEffect::SuperScalerPop => "super-scaler-pop",
        PreviewTransitionEffect::MaskBlit => "mask-blit",
        PreviewTransitionEffect::PhosphorDecay => "phosphor-decay",
        PreviewTransitionEffect::PlasmaMask => "plasma-mask",
        PreviewTransitionEffect::MoireRings => "moire-rings",
        PreviewTransitionEffect::KefrensCurtain => "kefrens-curtain",
    }
}

pub(crate) fn parse(value: &str) -> Option<PreviewTransitionEffect> {
    match value {
        "wipe" | "wipe-left" => Some(PreviewTransitionEffect::Wipe),
        "slide" | "slide-left" => Some(PreviewTransitionEffect::Slide),
        "zoom" | "pop" => Some(PreviewTransitionEffect::Zoom),
        "scanline" | "scanlines" | "scan" => Some(PreviewTransitionEffect::Scanline),
        "checker" | "checkerboard" => Some(PreviewTransitionEffect::Checker),
        "dissolve" | "noise" => Some(PreviewTransitionEffect::Dissolve),
        "crt-beam" | "crt-beam-wipe" | "beam" | "beam-wipe" => {
            Some(PreviewTransitionEffect::CrtBeamWipe)
        }
        "mosaic" | "mosaic-resolve" | "chunky" | "chunky-resolve" => {
            Some(PreviewTransitionEffect::MosaicResolve)
        }
        "copper" | "copper-bars" => Some(PreviewTransitionEffect::CopperBars),
        "venetian" | "venetian-blinds" | "blinds" => Some(PreviewTransitionEffect::VenetianBlinds),
        "barn" | "barn-door" | "barn-doors" => Some(PreviewTransitionEffect::BarnDoor),
        "iris" | "circle" => Some(PreviewTransitionEffect::Iris),
        "clock" | "clock-wipe" | "radar" => Some(PreviewTransitionEffect::ClockWipe),
        "sprite-strips" | "strips" => Some(PreviewTransitionEffect::SpriteStrips),
        "starfield" | "starfield-warp" | "warp-stars" => {
            Some(PreviewTransitionEffect::StarfieldWarp)
        }
        "vector" | "vector-redraw" => Some(PreviewTransitionEffect::VectorRedraw),
        "palette" | "palette-cycle" | "cycle" => Some(PreviewTransitionEffect::PaletteCycle),
        "raster-tear" | "tear" => Some(PreviewTransitionEffect::RasterTear),
        "tile-loader" | "tile-load" | "loader" => Some(PreviewTransitionEffect::TileLoader),
        "venetian-copper" | "copper-blinds" => Some(PreviewTransitionEffect::VenetianCopper),
        "attribute" | "attribute-flash" | "color-clash" => {
            Some(PreviewTransitionEffect::AttributeFlash)
        }
        "tec-tec" | "tectec" => Some(PreviewTransitionEffect::TecTec),
        "linecrunch" | "line-crunch" => Some(PreviewTransitionEffect::Linecrunch),
        "racing-beam" | "race-beam" => Some(PreviewTransitionEffect::RacingBeam),
        "sprite-multiplex" | "multiplex" => Some(PreviewTransitionEffect::SpriteMultiplex),
        "row-scroll-parallax" | "parallax" => Some(PreviewTransitionEffect::RowScrollParallax),
        "super-scaler-pop" | "superscaler" | "scaler-pop" => {
            Some(PreviewTransitionEffect::SuperScalerPop)
        }
        "mask-blit" | "mask" => Some(PreviewTransitionEffect::MaskBlit),
        "phosphor" | "phosphor-decay" => Some(PreviewTransitionEffect::PhosphorDecay),
        "plasma" | "plasma-mask" => Some(PreviewTransitionEffect::PlasmaMask),
        "moire" | "moire-rings" => Some(PreviewTransitionEffect::MoireRings),
        "kefrens" | "kefrens-curtain" | "curtain" => Some(PreviewTransitionEffect::KefrensCurtain),
        _ => None,
    }
}
