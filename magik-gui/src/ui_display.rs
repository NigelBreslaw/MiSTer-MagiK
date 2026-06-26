//! Menu-output framebuffer vs Slint render buffer.
//!
//! Slint layouts stay at **`MisterUi.scale = 1`**. The launcher chooses a
//! framebuffer from MiSTer.ini's Menu output mode and lets the MiSTer FPGA scale
//! it to the final HDMI/direct-video rectangle.

/// Slint global — always 1; layout math uses base units only.
pub const SLINT_UI_SCALE: i32 = 1;

const MISTER_INI_PATH: &str = "/media/fat/MiSTer.ini";
pub const DEFAULT_OUTPUT_W: u16 = 1920;
pub const DEFAULT_OUTPUT_H: u16 = 1080;
pub const UI_FB_W: usize = 960;
pub const UI_FB_H: usize = 540;
const MIN_RUNTIME_SCAN_W: u16 = 320;
const MIN_RUNTIME_SCAN_H: u16 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiDisplayPlan {
    pub fb_w: usize,
    pub fb_h: usize,
    pub output_w: u16,
    pub output_h: u16,
    pub scan_w: u16,
    pub scan_h: u16,
    pub direct_video: bool,
    pub source: &'static str,
    pub fallback: bool,
}

impl UiDisplayPlan {
    pub fn from_runtime_or_mister_ini_file(runtime: Option<RuntimeDisplayGeometry>) -> Self {
        let ini = std::fs::read_to_string(MISTER_INI_PATH).ok();
        if let Some(runtime) = runtime {
            return Self::from_runtime_geometry(
                runtime,
                ini.as_deref()
                    .is_some_and(Self::direct_video_policy_from_mister_ini_text),
            );
        }
        ini.and_then(|ini| Self::from_mister_ini_text(&ini))
            .unwrap_or_else(Self::fallback_1080p)
    }

    pub fn from_mister_ini_text(ini: &str) -> Option<Self> {
        let parsed = ParsedIni::parse(ini);
        Self::from_parsed_mister_ini(&parsed)
    }

    #[cfg(test)]
    pub fn from_runtime_or_mister_ini_text(
        runtime: Option<RuntimeDisplayGeometry>,
        ini: &str,
    ) -> Option<Self> {
        if let Some(runtime) = runtime {
            return Some(Self::from_runtime_geometry(
                runtime,
                Self::direct_video_policy_from_mister_ini_text(ini),
            ));
        }
        Self::from_mister_ini_text(ini)
    }

    pub fn direct_video_policy_from_mister_ini_text(ini: &str) -> bool {
        direct_video_from_parsed(&ParsedIni::parse(ini))
    }

    fn from_parsed_mister_ini(parsed: &ParsedIni<'_>) -> Option<Self> {
        let direct_video = direct_video_from_parsed(parsed);
        if direct_video {
            let pal = parsed
                .value("MiSTer", "menu_pal")
                .or_else(|| parsed.value("global", "menu_pal"))
                .is_some_and(|value| value == "1");
            let scandoubler = parsed
                .value("MiSTer", "forced_scandoubler")
                .or_else(|| parsed.value("global", "forced_scandoubler"))
                .is_some_and(|value| value == "1");
            let (output_w, output_h) = match (pal, scandoubler) {
                (false, false) => (640, 240),
                (false, true) => (640, 480),
                (true, false) => (640, 288),
                (true, true) => (640, 576),
            };
            return Some(Self::from_output(
                output_w,
                output_h,
                true,
                "mister-ini-direct-video",
            ));
        }

        let video_mode = parsed
            .value("Menu", "video_mode")
            .or_else(|| parsed.value("MiSTer", "video_mode"))
            .or_else(|| parsed.value("global", "video_mode"))?;
        let geometry = video_mode_geometry(video_mode)?;
        Some(Self::from_geometry(
            geometry,
            false,
            "mister-ini-video-mode",
        ))
    }

    pub fn from_runtime_geometry(runtime: RuntimeDisplayGeometry, direct_video: bool) -> Self {
        Self::from_geometry(
            VideoModeGeometry::with_scan(
                runtime.output_w,
                runtime.output_h,
                runtime.scan_w,
                runtime.scan_h,
            ),
            direct_video,
            "runtime-video-info",
        )
    }

    pub fn fallback_1080p() -> Self {
        Self {
            fallback: true,
            ..Self::from_output(DEFAULT_OUTPUT_W, DEFAULT_OUTPUT_H, false, "fallback-1080p")
        }
    }

    fn from_output(output_w: u16, output_h: u16, direct_video: bool, source: &'static str) -> Self {
        Self::from_geometry(
            VideoModeGeometry::new(output_w, output_h),
            direct_video,
            source,
        )
    }

    fn from_geometry(
        geometry: VideoModeGeometry,
        direct_video: bool,
        source: &'static str,
    ) -> Self {
        let (fb_w, fb_h) =
            launcher_framebuffer_size(geometry.output_w as usize, geometry.output_h as usize);
        Self {
            fb_w,
            fb_h,
            output_w: geometry.output_w,
            output_h: geometry.output_h,
            scan_w: geometry.scan_w,
            scan_h: geometry.scan_h,
            direct_video,
            source,
            fallback: false,
        }
    }

    pub fn log_line(self) -> String {
        format!(
            "display-plan: source={} output={}x{} scan={}x{} fb={}x{} direct_video={} fallback={}",
            self.source,
            self.output_w,
            self.output_h,
            self.scan_w,
            self.scan_h,
            self.fb_w,
            self.fb_h,
            self.direct_video,
            self.fallback
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeDisplayGeometry {
    pub output_w: u16,
    pub output_h: u16,
    pub scan_w: u16,
    pub scan_h: u16,
}

impl RuntimeDisplayGeometry {
    pub fn from_video_words(width: u32, height: u32, de_h: u16, de_v: u16) -> Option<Self> {
        let scan_w = u16::try_from(width).ok().filter(|value| *value > 0)?;
        let scan_h = u16::try_from(height).ok().filter(|value| *value > 0)?;
        let output_w = if de_h > 0 { de_h } else { scan_w };
        let output_h = if de_v > 0 { de_v } else { scan_h };
        if scan_w < MIN_RUNTIME_SCAN_W
            || scan_h < MIN_RUNTIME_SCAN_H
            || output_w < MIN_RUNTIME_SCAN_W
            || output_h < MIN_RUNTIME_SCAN_H
        {
            return None;
        }
        Some(Self {
            output_w,
            output_h,
            scan_w,
            scan_h,
        })
    }
}

pub struct UiDisplay {
    fb_w: usize,
    fb_h: usize,
    render_w: usize,
    render_h: usize,
    output_w: u16,
    output_h: u16,
    scan_w: u16,
    scan_h: u16,
    direct_video: bool,
}

impl UiDisplay {
    #[allow(dead_code)]
    pub fn for_framebuffer(fb_w: usize, fb_h: usize) -> Self {
        Self {
            fb_w,
            fb_h,
            render_w: fb_w,
            render_h: fb_h,
            output_w: fb_w.min(u16::MAX as usize) as u16,
            output_h: fb_h.min(u16::MAX as usize) as u16,
            scan_w: fb_w.min(u16::MAX as usize) as u16,
            scan_h: fb_h.min(u16::MAX as usize) as u16,
            direct_video: false,
        }
    }

    pub fn for_plan(plan: UiDisplayPlan) -> Self {
        Self {
            fb_w: plan.fb_w,
            fb_h: plan.fb_h,
            render_w: plan.fb_w,
            render_h: plan.fb_h,
            output_w: plan.output_w,
            output_h: plan.output_h,
            scan_w: plan.scan_w,
            scan_h: plan.scan_h,
            direct_video: plan.direct_video,
        }
    }

    pub fn render_w(&self) -> usize {
        self.render_w
    }

    pub fn render_h(&self) -> usize {
        self.render_h
    }

    pub fn fb_w(&self) -> usize {
        self.fb_w
    }

    pub fn fb_h(&self) -> usize {
        self.fb_h
    }

    pub fn output_w(&self) -> u16 {
        self.output_w
    }

    pub fn output_h(&self) -> u16 {
        self.output_h
    }

    pub fn scan_w(&self) -> u16 {
        self.scan_w
    }

    pub fn scan_h(&self) -> u16 {
        self.scan_h
    }

    pub fn direct_video(&self) -> bool {
        self.direct_video
    }

    pub fn log_line(&self) -> String {
        format!(
            "slint-scale={SLINT_UI_SCALE} render={}x{} fb={}x{} output={}x{} scan={}x{} direct_video={}",
            self.render_w(),
            self.render_h(),
            self.fb_w,
            self.fb_h,
            self.output_w,
            self.output_h,
            self.scan_w,
            self.scan_h,
            self.direct_video
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VideoModeGeometry {
    output_w: u16,
    output_h: u16,
    scan_w: u16,
    scan_h: u16,
}

impl VideoModeGeometry {
    const fn new(output_w: u16, output_h: u16) -> Self {
        Self {
            output_w,
            output_h,
            scan_w: output_w,
            scan_h: output_h,
        }
    }

    const fn with_scan(output_w: u16, output_h: u16, scan_w: u16, scan_h: u16) -> Self {
        Self {
            output_w,
            output_h,
            scan_w,
            scan_h,
        }
    }
}

fn launcher_framebuffer_size(output_w: usize, output_h: usize) -> (usize, usize) {
    if output_w >= 1600 || output_h >= 900 {
        ((output_w / 2).max(1), (output_h / 2).max(1))
    } else {
        (output_w.max(1), output_h.max(1))
    }
}

fn video_mode_geometry(value: &str) -> Option<VideoModeGeometry> {
    let value = value.trim();
    if let Ok(mode) = value.parse::<usize>() {
        return predefined_video_mode(mode);
    }

    let parts = value
        .split(',')
        .filter_map(|part| part.trim().parse::<u16>().ok())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [w, _, _, _, h, ..] => Some(VideoModeGeometry::new(*w, *h)),
        [w, h, ..] => Some(VideoModeGeometry::new(*w, *h)),
        _ => None,
    }
}

fn predefined_video_mode(mode: usize) -> Option<VideoModeGeometry> {
    const MODES: &[VideoModeGeometry] = &[
        VideoModeGeometry::new(1280, 720),
        VideoModeGeometry::new(1024, 768),
        VideoModeGeometry::new(720, 480),
        VideoModeGeometry::new(720, 576),
        VideoModeGeometry::new(1280, 1024),
        VideoModeGeometry::new(800, 600),
        VideoModeGeometry::new(640, 480),
        VideoModeGeometry::new(1280, 720),
        VideoModeGeometry::new(1920, 1080),
        VideoModeGeometry::new(1920, 1080),
        VideoModeGeometry::new(1366, 768),
        VideoModeGeometry::new(1024, 600),
        VideoModeGeometry::new(1920, 1440),
        VideoModeGeometry::new(2048, 1536),
        VideoModeGeometry::with_scan(2560, 1440, 1280, 1440),
    ];
    MODES.get(mode).copied()
}

fn direct_video_from_parsed(parsed: &ParsedIni<'_>) -> bool {
    parsed
        .value("MiSTer", "direct_video")
        .or_else(|| parsed.value("global", "direct_video"))
        .is_some_and(|value| value == "1" || value == "2")
}

struct ParsedIni<'a> {
    entries: Vec<(&'a str, &'a str, &'a str)>,
}

impl<'a> ParsedIni<'a> {
    fn parse(text: &'a str) -> Self {
        let mut section = "global";
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            entries.push((section, key.trim(), value.trim()));
        }
        Self { entries }
    }

    fn value(&self, section: &str, key: &str) -> Option<&'a str> {
        self.entries
            .iter()
            .rev()
            .find(|(entry_section, entry_key, _)| {
                entry_section.eq_ignore_ascii_case(section) && entry_key.eq_ignore_ascii_case(key)
            })
            .map(|(_, _, value)| *value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_plan_halves_hd_modes() {
        let plan = UiDisplayPlan::from_output(1920, 1080, false, "test");
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
        let ui = UiDisplay::for_plan(plan);
        assert_eq!(ui.render_w(), 960);
        assert_eq!(ui.render_h(), 540);
    }

    #[test]
    fn display_plan_keeps_720p_and_lower_native() {
        assert_eq!(launcher_framebuffer_size(1280, 720), (1280, 720));
        assert_eq!(launcher_framebuffer_size(720, 480), (720, 480));
        assert_eq!(launcher_framebuffer_size(640, 480), (640, 480));
        assert_eq!(launcher_framebuffer_size(640, 240), (640, 240));
        assert_eq!(launcher_framebuffer_size(640, 288), (640, 288));
    }

    #[test]
    fn display_plan_halves_larger_modes() {
        assert_eq!(launcher_framebuffer_size(1920, 1440), (960, 720));
        assert_eq!(launcher_framebuffer_size(2048, 1536), (1024, 768));
    }

    #[test]
    fn parses_predefined_and_custom_video_modes() {
        assert_eq!(
            video_mode_geometry("8"),
            Some(VideoModeGeometry::new(1920, 1080))
        );
        assert_eq!(
            video_mode_geometry("0"),
            Some(VideoModeGeometry::new(1280, 720))
        );
        assert_eq!(
            video_mode_geometry("1280,110,40,220,720,5,5,20,74250"),
            Some(VideoModeGeometry::new(1280, 720))
        );
        assert_eq!(
            video_mode_geometry("1920,1200,60"),
            Some(VideoModeGeometry::new(1920, 1200))
        );
    }

    #[test]
    fn mode_14_uses_physical_output_size_but_pixel_repeat_scan_width() {
        let plan = UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=14\n").expect("plan");
        assert_eq!((plan.output_w, plan.output_h), (2560, 1440));
        assert_eq!((plan.scan_w, plan.scan_h), (1280, 1440));
        assert_eq!((plan.fb_w, plan.fb_h), (1280, 720));
    }

    #[test]
    fn parsed_ini_uses_last_matching_section_key() {
        let ini = "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=6\nvideo_mode=8 ; current\n";
        let parsed = ParsedIni::parse(ini);
        assert_eq!(parsed.value("Menu", "video_mode"), Some("8"));
        assert_eq!(parsed.value("MiSTer", "direct_video"), Some("0"));
    }

    #[test]
    fn plan_reads_menu_video_mode_from_ini_text() {
        let ini = "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=8\n";
        let plan = UiDisplayPlan::from_mister_ini_text(ini).expect("plan");
        assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
        assert!(!plan.direct_video);
        assert!(!plan.fallback);
    }

    #[test]
    fn detected_geometry_wins_over_ini_geometry() {
        let runtime = RuntimeDisplayGeometry::from_video_words(1280, 720, 1280, 720);
        let ini = "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=8\n";
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(runtime, ini).expect("plan");

        assert_eq!(plan.source, "runtime-video-info");
        assert_eq!((plan.output_w, plan.output_h), (1280, 720));
        assert_eq!((plan.fb_w, plan.fb_h), (1280, 720));
        assert!(!plan.direct_video);
    }

    #[test]
    fn detected_geometry_preserves_ini_direct_video_policy() {
        let runtime = RuntimeDisplayGeometry::from_video_words(640, 480, 640, 480);
        let ini =
            "[MiSTer]\ndirect_video=1\nmenu_pal=1\nforced_scandoubler=1\n[Menu]\nvideo_mode=8\n";
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(runtime, ini).expect("plan");

        assert_eq!(plan.source, "runtime-video-info");
        assert_eq!((plan.output_w, plan.output_h), (640, 480));
        assert!(plan.direct_video);
    }

    #[test]
    fn ini_geometry_is_fallback_when_detection_fails() {
        let ini = "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=8\n";
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(None, ini).expect("plan");

        assert_eq!(plan.source, "mister-ini-video-mode");
        assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
    }

    #[test]
    fn custom_ini_geometry_stays_compatible_as_fallback() {
        let ini = "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=1280,110,40,220,720,5,5,20,74250\n";
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(None, ini).expect("plan");

        assert_eq!(plan.source, "mister-ini-video-mode");
        assert_eq!((plan.output_w, plan.output_h), (1280, 720));
        assert_eq!((plan.fb_w, plan.fb_h), (1280, 720));
    }

    #[test]
    fn detected_pixel_repeat_geometry_uses_de_as_output_and_width_as_scan() {
        let runtime =
            RuntimeDisplayGeometry::from_video_words(1280, 1440, 2560, 1440).expect("runtime");
        let plan = UiDisplayPlan::from_runtime_geometry(runtime, false);

        assert_eq!((plan.output_w, plan.output_h), (2560, 1440));
        assert_eq!((plan.scan_w, plan.scan_h), (1280, 1440));
        assert_eq!((plan.fb_w, plan.fb_h), (1280, 720));
    }

    #[test]
    fn invalid_tiny_runtime_geometry_is_rejected_for_ini_fallback() {
        assert_eq!(
            RuntimeDisplayGeometry::from_video_words(192, 30, 192, 30),
            None
        );
    }

    #[test]
    fn plan_reads_direct_video_ntsc_and_pal_modes() {
        let ntsc = UiDisplayPlan::from_mister_ini_text(
            "[MiSTer]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
        )
        .expect("ntsc plan");
        assert_eq!((ntsc.output_w, ntsc.output_h), (640, 240));
        assert_eq!((ntsc.fb_w, ntsc.fb_h), (640, 240));
        assert!(ntsc.direct_video);

        let pal31 = UiDisplayPlan::from_mister_ini_text(
            "[MiSTer]\ndirect_video=1\nmenu_pal=1\nforced_scandoubler=1\n",
        )
        .expect("pal plan");
        assert_eq!((pal31.output_w, pal31.output_h), (640, 576));
        assert_eq!((pal31.fb_w, pal31.fb_h), (640, 576));
        assert!(pal31.direct_video);
    }

    #[test]
    fn fallback_plan_keeps_current_production_geometry() {
        let plan = UiDisplayPlan::fallback_1080p();
        assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
        assert!(plan.fallback);
    }
}
