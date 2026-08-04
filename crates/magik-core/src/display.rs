// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable launcher display geometry and supported output modes.

pub const DEFAULT_OUTPUT_W: u16 = 1920;
pub const DEFAULT_OUTPUT_H: u16 = 1080;
pub const UI_FB_W: usize = 960;
pub const UI_FB_H: usize = 540;
pub const UI_FB_720P_W: usize = 1280;
pub const UI_FB_720P_H: usize = 720;
pub const CRT_COMPOSITION_W: usize = 640;
pub const CRT_COMPOSITION_H: usize = 480;
const MIN_RUNTIME_SCAN_W: u16 = 320;
const MIN_RUNTIME_SCAN_H: u16 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedOutputRoute {
    Hdmi,
    Crt240p60,
    Crt288p50,
    Crt480p60,
    Crt576p50,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentInsets {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
}

impl ResolvedOutputRoute {
    pub fn from_runtime_settings_v1(value: &str) -> Option<Self> {
        let mut schema = None;
        let mut output = None;
        for field in value.split('&') {
            let (key, value) = field.split_once('=')?;
            match key {
                "schema" if schema.replace(value).is_none() => {}
                "output" if output.replace(value).is_none() => {}
                _ => return None,
            }
        }
        if schema != Some("1") {
            return None;
        }
        match output? {
            "hdmi" => Some(Self::Hdmi),
            "crt-240p60" => Some(Self::Crt240p60),
            "crt-288p50" => Some(Self::Crt288p50),
            "crt-480p60" => Some(Self::Crt480p60),
            "crt-576p50" => Some(Self::Crt576p50),
            _ => None,
        }
    }

    pub const fn is_crt(self) -> bool {
        !matches!(self, Self::Hdmi)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Hdmi => "hdmi",
            Self::Crt240p60 => "crt-240p60",
            Self::Crt288p50 => "crt-288p50",
            Self::Crt480p60 => "crt-480p60",
            Self::Crt576p50 => "crt-576p50",
        }
    }

    pub const fn nominal_period_us(self) -> Option<u64> {
        match self {
            Self::Hdmi => None,
            Self::Crt240p60 => Some(16_652),
            Self::Crt288p50 => Some(19_830),
            Self::Crt480p60 => Some(16_683),
            Self::Crt576p50 => Some(19_829),
        }
    }

    pub const fn content_insets(self) -> ContentInsets {
        match self {
            Self::Crt288p50 => ContentInsets {
                top: 20,
                bottom: 13,
                left: 0,
                right: 0,
            },
            Self::Crt576p50 => ContentInsets {
                right: 64,
                left: 0,
                top: 0,
                bottom: 0,
            },
            _ => ContentInsets {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
        }
    }

    pub const fn progressive_geometry(self) -> Option<DisplayGeometry> {
        match self {
            Self::Hdmi => None,
            Self::Crt240p60 => Some(DisplayGeometry::new(640, 240)),
            Self::Crt288p50 => Some(DisplayGeometry::new(640, 288)),
            Self::Crt480p60 => Some(DisplayGeometry::new(640, 480)),
            Self::Crt576p50 => Some(DisplayGeometry::new(640, 576)),
        }
    }

    pub const fn framebuffer_geometry(self) -> Option<(usize, usize)> {
        match self {
            Self::Hdmi => None,
            Self::Crt240p60 => Some((640, 240)),
            Self::Crt288p50 => Some((640, 288)),
            Self::Crt480p60 => Some((640, 480)),
            Self::Crt576p50 => Some((640, 576)),
        }
    }

    pub const fn composition_geometry(self, framebuffer: (usize, usize)) -> (usize, usize) {
        match self {
            Self::Crt288p50 | Self::Crt576p50 => framebuffer,
            Self::Crt240p60 | Self::Crt480p60 => (CRT_COMPOSITION_W, CRT_COMPOSITION_H),
            Self::Hdmi => framebuffer,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayGeometry {
    pub output_w: u16,
    pub output_h: u16,
    pub scan_w: u16,
    pub scan_h: u16,
}

impl DisplayGeometry {
    pub const fn new(output_w: u16, output_h: u16) -> Self {
        Self::with_scan(output_w, output_h, output_w, output_h)
    }

    pub const fn with_scan(output_w: u16, output_h: u16, scan_w: u16, scan_h: u16) -> Self {
        Self {
            output_w,
            output_h,
            scan_w,
            scan_h,
        }
    }

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
        Some(Self::with_scan(output_w, output_h, scan_w, scan_h))
    }
}

pub type RuntimeDisplayGeometry = DisplayGeometry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferSizePolicy {
    Auto,
    Force960x540,
    Force1280x720,
}

impl FramebufferSizePolicy {
    pub const fn framebuffer_size(self, output_w: usize, output_h: usize) -> (usize, usize) {
        match self {
            Self::Auto => launcher_framebuffer_size(output_w, output_h),
            Self::Force960x540 => (UI_FB_W, UI_FB_H),
            Self::Force1280x720 => (UI_FB_720P_W, UI_FB_720P_H),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedDisplayPlan {
    pub fb_w: usize,
    pub fb_h: usize,
    pub render_w: usize,
    pub render_h: usize,
    pub output_w: u16,
    pub output_h: u16,
    pub scan_w: u16,
    pub scan_h: u16,
    pub output_route: ResolvedOutputRoute,
    pub fb_policy: FramebufferSizePolicy,
}

impl ResolvedDisplayPlan {
    pub fn from_geometry(
        geometry: DisplayGeometry,
        output_route: ResolvedOutputRoute,
        fb_policy: FramebufferSizePolicy,
    ) -> Self {
        let (fb_w, fb_h, fb_policy) = match output_route.framebuffer_geometry() {
            Some((fb_w, fb_h)) => (fb_w, fb_h, FramebufferSizePolicy::Auto),
            None => {
                let (fb_w, fb_h) = fb_policy
                    .framebuffer_size(geometry.output_w as usize, geometry.output_h as usize);
                (fb_w, fb_h, fb_policy)
            }
        };
        let (render_w, render_h) = output_route.composition_geometry((fb_w, fb_h));
        Self {
            fb_w,
            fb_h,
            render_w,
            render_h,
            output_w: geometry.output_w,
            output_h: geometry.output_h,
            scan_w: geometry.scan_w,
            scan_h: geometry.scan_h,
            output_route,
            fb_policy,
        }
    }

    pub fn from_mode_or_detected(
        mode_id: &str,
        detected: Option<RuntimeDisplayGeometry>,
    ) -> Option<Self> {
        let route = route_for_mode_id(mode_id)?;
        let geometry = if matches!(mode_id, "auto" | "custom") {
            detected?
        } else {
            geometry_for_mode_id(mode_id)?
        };
        Some(Self::from_geometry(
            geometry,
            route,
            FramebufferSizePolicy::Auto,
        ))
    }
}

pub const fn launcher_framebuffer_size(output_w: usize, output_h: usize) -> (usize, usize) {
    if output_w >= 1366 || output_h >= 900 {
        ((output_w / 2).max(1), (output_h / 2).max(1))
    } else {
        (output_w.max(1), output_h.max(1))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayResolution {
    pub id: &'static str,
    pub label: &'static str,
    pub output_w: u16,
    pub output_h: u16,
    pub video_mode: Option<&'static str>,
    pub direct_video: u8,
    pub menu_pal: u8,
    pub forced_scandoubler: u8,
}

pub const AUTOMATIC_DISPLAY_RESOLUTION: DisplayResolution = DisplayResolution {
    id: "auto",
    label: "Automatic (HDMI / VGA DAC)",
    output_w: 0,
    output_h: 0,
    video_mode: None,
    direct_video: 2,
    menu_pal: 0,
    forced_scandoubler: 0,
};

pub const DISPLAY_RESOLUTIONS: &[DisplayResolution] = &[
    DisplayResolution {
        id: "hdmi-1280x720p60",
        label: "1280x720 (16:9)",
        output_w: 1280,
        output_h: 720,
        video_mode: Some("0"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-1366x768p60",
        label: "1366x768 (16:9)",
        output_w: 1366,
        output_h: 768,
        video_mode: Some("10"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-1920x1080p60",
        label: "1920x1080 (16:9)",
        output_w: 1920,
        output_h: 1080,
        video_mode: Some("8"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-1920x1200p60",
        label: "1920x1200 (16:10)",
        output_w: 1920,
        output_h: 1200,
        video_mode: Some("1920,1200,60"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-2048x1536p60",
        label: "2048x1536 (4:3)",
        output_w: 2048,
        output_h: 1536,
        video_mode: Some("13"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "crt-240p60",
        label: "CRT 240p 60hz NTSC",
        output_w: 640,
        output_h: 240,
        video_mode: None,
        direct_video: 1,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "crt-480p60",
        label: "CRT 480p 60hz NTSC",
        output_w: 640,
        output_h: 480,
        video_mode: None,
        direct_video: 1,
        menu_pal: 0,
        forced_scandoubler: 1,
    },
    DisplayResolution {
        id: "crt-288p50",
        label: "CRT 288p 50hz PAL",
        output_w: 640,
        output_h: 288,
        video_mode: None,
        direct_video: 1,
        menu_pal: 1,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "crt-576p50",
        label: "CRT 576p 50hz PAL",
        output_w: 640,
        output_h: 576,
        video_mode: None,
        direct_video: 1,
        menu_pal: 1,
        forced_scandoubler: 1,
    },
];

pub fn find_display_resolution(id: &str) -> Option<&'static DisplayResolution> {
    if id == AUTOMATIC_DISPLAY_RESOLUTION.id {
        Some(&AUTOMATIC_DISPLAY_RESOLUTION)
    } else {
        DISPLAY_RESOLUTIONS.iter().find(|mode| mode.id == id)
    }
}

pub fn route_for_mode_id(id: &str) -> Option<ResolvedOutputRoute> {
    match id {
        "auto" | "custom" | "hdmi-1280x720p60" | "hdmi-1366x768p60" | "hdmi-1920x1080p60"
        | "hdmi-1920x1200p60" | "hdmi-2048x1536p60" | "hdmi-2560x1440p60" => {
            Some(ResolvedOutputRoute::Hdmi)
        }
        "crt-240p60" => Some(ResolvedOutputRoute::Crt240p60),
        "crt-288p50" => Some(ResolvedOutputRoute::Crt288p50),
        "crt-480p60" => Some(ResolvedOutputRoute::Crt480p60),
        "crt-576p50" => Some(ResolvedOutputRoute::Crt576p50),
        _ => None,
    }
}

pub fn geometry_for_mode_id(id: &str) -> Option<DisplayGeometry> {
    match id {
        "hdmi-1280x720p60" => Some(DisplayGeometry::new(1280, 720)),
        "hdmi-1366x768p60" => Some(DisplayGeometry::new(1366, 768)),
        "hdmi-1920x1080p60" => Some(DisplayGeometry::new(1920, 1080)),
        "hdmi-1920x1200p60" => Some(DisplayGeometry::new(1920, 1200)),
        "hdmi-2048x1536p60" => Some(DisplayGeometry::new(2048, 1536)),
        "hdmi-2560x1440p60" => Some(DisplayGeometry::with_scan(2560, 1440, 1280, 1440)),
        "crt-240p60" => Some(DisplayGeometry::new(640, 240)),
        "crt-288p50" => Some(DisplayGeometry::new(640, 288)),
        "crt-480p60" => Some(DisplayGeometry::new(640, 480)),
        "crt-576p50" => Some(DisplayGeometry::new(640, 576)),
        "auto" | "custom" => None,
        _ => None,
    }
}

pub fn runtime_display_geometry_v1(value: &str) -> Option<DisplayGeometry> {
    let mut schema = None;
    let mut mode = None;
    for field in value.split('&') {
        let (key, value) = field.split_once('=')?;
        match key {
            "schema" if schema.replace(value).is_none() => {}
            "mode" if mode.replace(value).is_none() => {}
            _ => return None,
        }
    }
    if schema != Some("1") {
        return None;
    }
    geometry_for_mode_id(mode?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_geometry_matches_product_contract() {
        for (mode, fb, render) in [
            ("hdmi-1280x720p60", (1280, 720), (1280, 720)),
            ("hdmi-1366x768p60", (683, 384), (683, 384)),
            ("hdmi-1920x1080p60", (960, 540), (960, 540)),
            ("hdmi-1920x1200p60", (960, 600), (960, 600)),
            ("hdmi-2048x1536p60", (1024, 768), (1024, 768)),
            ("hdmi-2560x1440p60", (1280, 720), (1280, 720)),
            ("crt-240p60", (640, 240), (640, 480)),
            ("crt-288p50", (640, 288), (640, 288)),
            ("crt-480p60", (640, 480), (640, 480)),
            ("crt-576p50", (640, 576), (640, 576)),
        ] {
            let plan = ResolvedDisplayPlan::from_mode_or_detected(mode, None).expect(mode);
            assert_eq!((plan.fb_w, plan.fb_h), fb, "{mode}");
            assert_eq!((plan.render_w, plan.render_h), render, "{mode}");
        }
    }

    #[test]
    fn automatic_and_custom_reuse_detected_geometry() {
        let detected = DisplayGeometry::with_scan(1920, 1200, 1920, 1200);
        for mode in ["auto", "custom"] {
            let plan = ResolvedDisplayPlan::from_mode_or_detected(mode, Some(detected)).unwrap();
            assert_eq!((plan.render_w, plan.render_h), (960, 600));
        }
        assert!(ResolvedDisplayPlan::from_mode_or_detected("auto", None).is_none());
    }

    #[test]
    fn runtime_contract_is_strict() {
        assert_eq!(
            runtime_display_geometry_v1("schema=1&mode=hdmi-1920x1200p60"),
            Some(DisplayGeometry::new(1920, 1200))
        );
        for invalid in [
            "schema=2&mode=hdmi-1920x1080p60",
            "schema=1&mode=auto",
            "schema=1&mode=custom",
            "schema=1&mode=unsafe",
            "schema=1&mode=hdmi-1920x1080p60&extra=1",
        ] {
            assert!(runtime_display_geometry_v1(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn selectable_catalog_stays_stable() {
        assert_eq!(DISPLAY_RESOLUTIONS.len(), 9);
        assert!(DISPLAY_RESOLUTIONS.iter().all(|mode| mode.id != "auto"));
        assert!(
            DISPLAY_RESOLUTIONS
                .iter()
                .all(|mode| mode.id != "hdmi-2560x1440p60")
        );
        assert_eq!(
            find_display_resolution("auto"),
            Some(&AUTOMATIC_DISPLAY_RESOLUTION)
        );
    }

    #[test]
    fn runtime_geometry_uses_de_for_pixel_repeated_output() {
        assert_eq!(
            DisplayGeometry::from_video_words(1280, 1440, 2560, 1440),
            Some(DisplayGeometry::with_scan(2560, 1440, 1280, 1440))
        );
        assert!(DisplayGeometry::from_video_words(192, 30, 192, 30).is_none());
    }
}
