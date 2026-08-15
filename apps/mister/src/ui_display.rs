// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Menu-output framebuffer vs Slint render buffer.
//!
//! Slint layouts use framebuffer pixels directly. The launcher chooses a
//! framebuffer from MiSTer.ini's Menu output mode and lets the MiSTer FPGA
//! scale it to the final HDMI/direct-video rectangle.

#[cfg(test)]
use mister_magik_core::display::launcher_framebuffer_size;
pub use mister_magik_core::display::{
    DEFAULT_OUTPUT_H, DEFAULT_OUTPUT_W, ResolvedOutputRoute, RuntimeDisplayGeometry, UI_FB_H,
    UI_FB_W,
};
use mister_magik_core::display::{
    DisplayGeometry as VideoModeGeometry, FramebufferSizePolicy, ResolvedDisplayPlan,
    runtime_display_geometry_v1, video_mode_geometry,
};
use mister_magik_framebuffer_scenes::{OutputRotation, Rgb565OutputLayout, Rgb565Rect};
use mister_magik_mister_runtime::framebuffer::damage::DirtyRect;
pub use mister_magik_mister_runtime::settings::ScreenOrientation;

const DEVICE_INI_PATH: &str = "/media/fat/MiSTer.ini";
const UI_FB_SIZE_ENV: &str = "MISTER_UI_FB_SIZE";
const RUNTIME_SETTINGS_ENV: &str = "MISTER_MAGIK_RUNTIME_SETTINGS_V1";
const RUNTIME_DISPLAY_ENV: &str = "MISTER_MAGIK_RUNTIME_DISPLAY_V1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrtFontFamily {
    PressStart2P,
}

impl CrtFontFamily {
    /// Slint family name embedded in the route's selected font asset.
    pub const fn label(self) -> &'static str {
        match self {
            Self::PressStart2P => "Press Start 2P",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiPixelSize {
    Px8,
    Px16,
    Px24,
    Px32,
}

impl UiPixelSize {
    pub const fn pixels(self) -> i32 {
        match self {
            Self::Px8 => 8,
            Self::Px16 => 16,
            Self::Px24 => 24,
            Self::Px32 => 32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtUiMetrics {
    pub grid_x: i32,
    pub grid_y: i32,
    pub border_x: i32,
    pub border_y: i32,
    pub body_font: UiPixelSize,
    pub heading_font: UiPixelSize,
    pub card_title_font: UiPixelSize,
    pub card_detail_font: UiPixelSize,
    pub game_row_height: i32,
    pub header_height: i32,
    pub footer_height: i32,
    pub font_family: CrtFontFamily,
}

impl CrtUiMetrics {
    pub const fn for_framebuffer(_fb_w: usize, _fb_h: usize) -> Self {
        Self {
            grid_x: 4,
            grid_y: 4,
            border_x: 1,
            border_y: 1,
            body_font: UiPixelSize::Px8,
            heading_font: UiPixelSize::Px16,
            card_title_font: UiPixelSize::Px16,
            card_detail_font: UiPixelSize::Px8,
            game_row_height: 24,
            header_height: 48,
            footer_height: 24,
            font_family: CrtFontFamily::PressStart2P,
        }
    }

    pub const fn for_display(display: &UiDisplay) -> Self {
        match display.output_route {
            ResolvedOutputRoute::Crt240p60 => Self {
                grid_x: 8,
                grid_y: 8,
                border_x: 2,
                border_y: 2,
                body_font: UiPixelSize::Px16,
                heading_font: UiPixelSize::Px32,
                card_title_font: UiPixelSize::Px24,
                card_detail_font: UiPixelSize::Px16,
                game_row_height: 32,
                header_height: 80,
                footer_height: 40,
                font_family: CrtFontFamily::PressStart2P,
            },
            ResolvedOutputRoute::Crt288p50 => Self {
                grid_x: 8,
                grid_y: 5,
                border_x: 2,
                border_y: 1,
                body_font: UiPixelSize::Px16,
                heading_font: UiPixelSize::Px32,
                card_title_font: UiPixelSize::Px24,
                card_detail_font: UiPixelSize::Px16,
                game_row_height: 19,
                header_height: 56,
                footer_height: 24,
                font_family: CrtFontFamily::PressStart2P,
            },
            ResolvedOutputRoute::Crt480p60 => Self {
                grid_x: 4,
                grid_y: 4,
                border_x: 1,
                border_y: 1,
                body_font: UiPixelSize::Px8,
                heading_font: UiPixelSize::Px16,
                card_title_font: UiPixelSize::Px16,
                card_detail_font: UiPixelSize::Px8,
                game_row_height: 32,
                header_height: 48,
                footer_height: 24,
                font_family: CrtFontFamily::PressStart2P,
            },
            ResolvedOutputRoute::Crt576p50 => Self {
                grid_x: 4,
                grid_y: 5,
                border_x: 1,
                border_y: 1,
                body_font: UiPixelSize::Px8,
                heading_font: UiPixelSize::Px16,
                card_title_font: UiPixelSize::Px16,
                card_detail_font: UiPixelSize::Px8,
                game_row_height: 39,
                header_height: 56,
                footer_height: 29,
                font_family: CrtFontFamily::PressStart2P,
            },
            _ => Self::for_framebuffer(display.render_w, display.render_h),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtContentRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// Launcher layout geometry above the fixed display/composition geometry.
///
/// Portrait layouts swap the composition axes. They are rotated into the
/// landscape composition buffer before the existing output transform runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiLayoutGeometry {
    orientation: ScreenOrientation,
    output_layout: Rgb565OutputLayout,
    logical_w: usize,
    logical_h: usize,
    composition_w: usize,
    composition_h: usize,
    content_rect: CrtContentRect,
}

impl UiLayoutGeometry {
    pub fn for_display(display: &UiDisplay, orientation: ScreenOrientation) -> Self {
        let composition_w = display.render_w();
        let composition_h = display.render_h();
        let (logical_w, logical_h) = if orientation.is_portrait() {
            (composition_h, composition_w)
        } else {
            (composition_w, composition_h)
        };
        let content_rect = Self::composition_rect_to_logical(
            orientation,
            logical_w,
            logical_h,
            display.content_rect(),
        );
        let output_layout = Rgb565OutputLayout::new(
            logical_w,
            logical_h,
            composition_w,
            Self::output_rotation(orientation),
        )
        .expect("display-derived UI geometry is a valid RGB565 output layout");
        Self {
            orientation,
            output_layout,
            logical_w,
            logical_h,
            composition_w,
            composition_h,
            content_rect,
        }
    }

    pub const fn orientation(self) -> ScreenOrientation {
        self.orientation
    }

    pub const fn is_portrait(self) -> bool {
        self.orientation.is_portrait()
    }

    pub const fn logical_w(self) -> usize {
        self.logical_w
    }

    pub const fn logical_h(self) -> usize {
        self.logical_h
    }

    pub const fn composition_w(self) -> usize {
        self.composition_w
    }

    pub const fn composition_h(self) -> usize {
        self.composition_h
    }

    pub const fn content_rect(self) -> CrtContentRect {
        self.content_rect
    }

    pub const fn output_layout(self) -> Rgb565OutputLayout {
        self.output_layout
    }

    /// Maps a logical pixel coordinate into the persistent composition cache.
    pub fn logical_pixel_to_composition(self, x: usize, y: usize) -> (usize, usize) {
        self.output_layout.logical_to_physical(x, y)
    }

    pub fn composition_pixel_to_logical(self, x: usize, y: usize) -> (usize, usize) {
        self.output_layout.physical_to_logical(x, y)
    }

    pub fn logical_rect_to_composition(self, rect: DirtyRect) -> DirtyRect {
        let mapped = self.output_layout.logical_rect_to_physical(Rgb565Rect {
            x0: rect.x0,
            y0: rect.y0,
            x1: rect.x1,
            y1: rect.y1,
        });
        DirtyRect {
            x0: mapped.x0,
            y0: mapped.y0,
            x1: mapped.x1,
            y1: mapped.y1,
        }
    }

    pub fn composition_rect_to_logical_rect(self, rect: DirtyRect) -> DirtyRect {
        let rect = DirtyRect {
            x0: rect.x0.min(self.composition_w),
            y0: rect.y0.min(self.composition_h),
            x1: rect.x1.min(self.composition_w),
            y1: rect.y1.min(self.composition_h),
        };
        match self.orientation {
            ScreenOrientation::Normal => rect,
            ScreenOrientation::MonitorClockwise => DirtyRect {
                x0: self.logical_w - rect.y1,
                y0: rect.x0,
                x1: self.logical_w - rect.y0,
                y1: rect.x1,
            },
            ScreenOrientation::MonitorCounterclockwise => DirtyRect {
                x0: rect.y0,
                y0: self.logical_h - rect.x1,
                x1: rect.y1,
                y1: self.logical_h - rect.x0,
            },
        }
    }

    const fn output_rotation(orientation: ScreenOrientation) -> OutputRotation {
        match orientation {
            ScreenOrientation::Normal => OutputRotation::None,
            // A clockwise-mounted monitor requires counterclockwise output.
            ScreenOrientation::MonitorClockwise => OutputRotation::CounterClockwise90,
            // A counterclockwise-mounted monitor requires clockwise output.
            ScreenOrientation::MonitorCounterclockwise => OutputRotation::Clockwise90,
        }
    }

    fn composition_rect_to_logical(
        orientation: ScreenOrientation,
        logical_w: usize,
        logical_h: usize,
        rect: CrtContentRect,
    ) -> CrtContentRect {
        let mapped = Self {
            orientation,
            output_layout: Rgb565OutputLayout::new(
                logical_w,
                logical_h,
                if orientation.is_portrait() {
                    logical_h
                } else {
                    logical_w
                },
                Self::output_rotation(orientation),
            )
            .expect("content mapping uses valid UI geometry"),
            logical_w,
            logical_h,
            composition_w: if orientation.is_portrait() {
                logical_h
            } else {
                logical_w
            },
            composition_h: if orientation.is_portrait() {
                logical_w
            } else {
                logical_h
            },
            content_rect: rect,
        }
        .composition_rect_to_logical_rect(DirtyRect {
            x0: rect.x,
            y0: rect.y,
            x1: rect.right(),
            y1: rect.bottom(),
        });
        CrtContentRect {
            x: mapped.x0,
            y: mapped.y0,
            width: mapped.width(),
            height: mapped.rows() as usize,
        }
    }
}

impl CrtContentRect {
    pub const fn right(self) -> usize {
        self.x + self.width
    }

    pub const fn bottom(self) -> usize {
        self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiFramebufferSizePolicy {
    Auto,
    Force960x540,
    Force1280x720,
}

impl UiFramebufferSizePolicy {
    pub fn from_env() -> Self {
        std::env::var(UI_FB_SIZE_ENV)
            .ok()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or(Self::Auto)
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Some(Self::Auto),
            "960x540" => Some(Self::Force960x540),
            "1280x720" => Some(Self::Force1280x720),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Force960x540 => "forced-960x540",
            Self::Force1280x720 => "forced-1280x720",
        }
    }

    pub const fn env_name() -> &'static str {
        UI_FB_SIZE_ENV
    }

    const fn shared(self) -> FramebufferSizePolicy {
        match self {
            Self::Auto => FramebufferSizePolicy::Auto,
            Self::Force960x540 => FramebufferSizePolicy::Force960x540,
            Self::Force1280x720 => FramebufferSizePolicy::Force1280x720,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiDisplayPlan {
    pub fb_w: usize,
    pub fb_h: usize,
    pub render_w: usize,
    pub render_h: usize,
    pub output_w: u16,
    pub output_h: u16,
    pub scan_w: u16,
    pub scan_h: u16,
    pub direct_video: bool,
    pub output_route: ResolvedOutputRoute,
    pub fb_policy: UiFramebufferSizePolicy,
    pub source: &'static str,
    pub fallback: bool,
}

impl UiDisplayPlan {
    pub const fn uses_crt_ui(self) -> bool {
        self.output_route.is_crt()
    }

    pub fn from_runtime_or_mister_ini_file(runtime: Option<RuntimeDisplayGeometry>) -> Self {
        let ini = std::fs::read_to_string(DEVICE_INI_PATH).ok();
        let fb_policy = UiFramebufferSizePolicy::from_env();
        let resolved_route = std::env::var(RUNTIME_SETTINGS_ENV)
            .ok()
            .as_deref()
            .and_then(ResolvedOutputRoute::from_runtime_settings_v1)
            .unwrap_or(ResolvedOutputRoute::Hdmi);
        if let Some(geometry) = resolved_route.progressive_geometry() {
            return Self::from_geometry_with_route(
                geometry,
                resolved_route,
                "main-runtime-settings-crt",
                UiFramebufferSizePolicy::Auto,
            );
        }
        if let Some(geometry) = std::env::var(RUNTIME_DISPLAY_ENV)
            .ok()
            .as_deref()
            .and_then(runtime_display_geometry_v1)
        {
            return Self::from_geometry_with_route(
                geometry,
                resolved_route,
                "main-runtime-display-mode",
                fb_policy,
            );
        }
        if let Some(runtime) = runtime {
            return Self::from_runtime_geometry_with_policy(runtime, false, fb_policy);
        }
        ini.and_then(|ini| Self::from_mister_ini_hdmi_text_with_policy(&ini, fb_policy))
            .unwrap_or_else(|| Self::fallback_1080p_with_policy(fb_policy))
    }

    pub fn from_mister_ini_text(ini: &str) -> Option<Self> {
        Self::from_mister_ini_text_with_policy(ini, UiFramebufferSizePolicy::Auto)
    }

    pub fn from_mister_ini_text_with_policy(
        ini: &str,
        fb_policy: UiFramebufferSizePolicy,
    ) -> Option<Self> {
        let parsed = ParsedIni::parse(ini);
        Self::from_parsed_mister_ini(&parsed, fb_policy)
    }

    #[cfg(test)]
    pub fn from_runtime_or_mister_ini_text(
        runtime: Option<RuntimeDisplayGeometry>,
        ini: &str,
        runtime_settings: Option<&str>,
        runtime_display: Option<&str>,
    ) -> Option<Self> {
        let resolved_route = runtime_settings
            .and_then(ResolvedOutputRoute::from_runtime_settings_v1)
            .unwrap_or(ResolvedOutputRoute::Hdmi);
        if let Some(geometry) = resolved_route.progressive_geometry() {
            return Some(Self::from_geometry_with_route(
                geometry,
                resolved_route,
                "test-runtime-settings-crt",
                UiFramebufferSizePolicy::Auto,
            ));
        }
        if let Some(geometry) = runtime_display.and_then(runtime_display_geometry_v1) {
            return Some(Self::from_geometry_with_route(
                geometry,
                resolved_route,
                "test-runtime-display-mode",
                UiFramebufferSizePolicy::Auto,
            ));
        }
        if let Some(runtime) = runtime {
            return Some(Self::from_runtime_geometry(runtime, false));
        }
        Self::from_mister_ini_hdmi_text_with_policy(ini, UiFramebufferSizePolicy::Auto)
    }

    pub fn direct_video_policy_from_mister_ini_text(ini: &str) -> bool {
        direct_video_from_parsed(&ParsedIni::parse(ini))
    }

    fn from_parsed_mister_ini(
        parsed: &ParsedIni<'_>,
        fb_policy: UiFramebufferSizePolicy,
    ) -> Option<Self> {
        let direct_video = direct_video_from_parsed(parsed);
        if direct_video {
            let pal = parsed
                .value("Menu", "menu_pal")
                .or_else(|| parsed.value("MiSTer", "menu_pal"))
                .or_else(|| parsed.value("global", "menu_pal"))
                .is_some_and(|value| value == "1");
            let scandoubler = parsed
                .value("Menu", "forced_scandoubler")
                .or_else(|| parsed.value("MiSTer", "forced_scandoubler"))
                .or_else(|| parsed.value("global", "forced_scandoubler"))
                .is_some_and(|value| value == "1");
            let route = match (pal, scandoubler) {
                (false, false) => ResolvedOutputRoute::Crt240p60,
                (false, true) => ResolvedOutputRoute::Crt480p60,
                (true, false) => ResolvedOutputRoute::Crt288p50,
                (true, true) => ResolvedOutputRoute::Crt576p50,
            };
            return Some(Self::from_geometry_with_route(
                route.progressive_geometry()?,
                route,
                "mister-ini-direct-video",
                fb_policy,
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
            fb_policy,
        ))
    }

    fn from_mister_ini_hdmi_text_with_policy(
        ini: &str,
        fb_policy: UiFramebufferSizePolicy,
    ) -> Option<Self> {
        let parsed = ParsedIni::parse(ini);
        let video_mode = parsed
            .value("Menu", "video_mode")
            .or_else(|| parsed.value("MiSTer", "video_mode"))
            .or_else(|| parsed.value("global", "video_mode"))?;
        Some(Self::from_geometry(
            video_mode_geometry(video_mode)?,
            false,
            "mister-ini-hdmi-fallback",
            fb_policy,
        ))
    }

    pub fn from_runtime_geometry(runtime: RuntimeDisplayGeometry, direct_video: bool) -> Self {
        Self::from_runtime_geometry_with_policy(
            runtime,
            direct_video,
            UiFramebufferSizePolicy::Auto,
        )
    }

    pub fn from_runtime_geometry_with_policy(
        runtime: RuntimeDisplayGeometry,
        direct_video: bool,
        fb_policy: UiFramebufferSizePolicy,
    ) -> Self {
        Self::from_geometry(
            VideoModeGeometry::with_scan(
                runtime.output_w,
                runtime.output_h,
                runtime.scan_w,
                runtime.scan_h,
            ),
            direct_video,
            "runtime-video-info",
            fb_policy,
        )
    }

    pub fn fallback_1080p() -> Self {
        Self::fallback_1080p_with_policy(UiFramebufferSizePolicy::Auto)
    }

    fn fallback_1080p_with_policy(fb_policy: UiFramebufferSizePolicy) -> Self {
        Self {
            fallback: true,
            ..Self::from_output_with_policy(
                DEFAULT_OUTPUT_W,
                DEFAULT_OUTPUT_H,
                false,
                "fallback-1080p",
                fb_policy,
            )
        }
    }

    fn from_output(output_w: u16, output_h: u16, direct_video: bool, source: &'static str) -> Self {
        Self::from_output_with_policy(
            output_w,
            output_h,
            direct_video,
            source,
            UiFramebufferSizePolicy::Auto,
        )
    }

    fn from_output_with_policy(
        output_w: u16,
        output_h: u16,
        direct_video: bool,
        source: &'static str,
        fb_policy: UiFramebufferSizePolicy,
    ) -> Self {
        Self::from_geometry(
            VideoModeGeometry::new(output_w, output_h),
            direct_video,
            source,
            fb_policy,
        )
    }

    fn from_geometry(
        geometry: VideoModeGeometry,
        direct_video: bool,
        source: &'static str,
        fb_policy: UiFramebufferSizePolicy,
    ) -> Self {
        let output_route = if direct_video {
            ResolvedOutputRoute::Crt240p60
        } else {
            ResolvedOutputRoute::Hdmi
        };
        Self::from_geometry_with_route(geometry, output_route, source, fb_policy)
    }

    fn from_geometry_with_route(
        geometry: VideoModeGeometry,
        output_route: ResolvedOutputRoute,
        source: &'static str,
        fb_policy: UiFramebufferSizePolicy,
    ) -> Self {
        let shared = ResolvedDisplayPlan::from_geometry(geometry, output_route, fb_policy.shared());
        let fb_policy = match shared.fb_policy {
            FramebufferSizePolicy::Auto => UiFramebufferSizePolicy::Auto,
            FramebufferSizePolicy::Force960x540 => UiFramebufferSizePolicy::Force960x540,
            FramebufferSizePolicy::Force1280x720 => UiFramebufferSizePolicy::Force1280x720,
        };
        Self {
            fb_w: shared.fb_w,
            fb_h: shared.fb_h,
            render_w: shared.render_w,
            render_h: shared.render_h,
            output_w: shared.output_w,
            output_h: shared.output_h,
            scan_w: shared.scan_w,
            scan_h: shared.scan_h,
            direct_video: output_route.is_crt(),
            output_route,
            fb_policy,
            source,
            fallback: false,
        }
    }

    pub fn log_line(self) -> String {
        format!(
            "display-plan: source={} route={} output={}x{} scan={}x{} render={}x{} fb={}x{} composition_transformed={} fb_policy={} direct_video={} fallback={}",
            self.source,
            self.output_route.label(),
            self.output_w,
            self.output_h,
            self.scan_w,
            self.scan_h,
            self.render_w,
            self.render_h,
            self.fb_w,
            self.fb_h,
            self.render_w != self.fb_w || self.render_h != self.fb_h,
            self.fb_policy.label(),
            self.direct_video,
            self.fallback
        )
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
    output_route: ResolvedOutputRoute,
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
            output_route: ResolvedOutputRoute::Hdmi,
        }
    }

    pub fn for_plan(plan: UiDisplayPlan) -> Self {
        Self {
            fb_w: plan.fb_w,
            fb_h: plan.fb_h,
            render_w: plan.render_w,
            render_h: plan.render_h,
            output_w: plan.output_w,
            output_h: plan.output_h,
            scan_w: plan.scan_w,
            scan_h: plan.scan_h,
            direct_video: plan.direct_video,
            output_route: plan.output_route,
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

    pub fn output_route(&self) -> ResolvedOutputRoute {
        self.output_route
    }

    pub fn content_rect(&self) -> CrtContentRect {
        let insets = self.output_route.content_insets();
        CrtContentRect {
            x: insets.left.min(self.render_w),
            y: insets.top.min(self.render_h),
            width: self
                .render_w
                .saturating_sub(insets.left.saturating_add(insets.right)),
            height: self
                .render_h
                .saturating_sub(insets.top.saturating_add(insets.bottom)),
        }
    }

    pub fn log_line(&self) -> String {
        format!(
            "render={}x{} fb={}x{} output={}x{} scan={}x{} direct_video={}",
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

fn direct_video_from_parsed(parsed: &ParsedIni<'_>) -> bool {
    parsed
        .value("Menu", "direct_video")
        .or_else(|| parsed.value("MiSTer", "direct_video"))
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
    fn crt_routes_expose_main_raster_periods() {
        assert_eq!(ResolvedOutputRoute::Hdmi.nominal_period_us(), None);
        assert_eq!(
            ResolvedOutputRoute::Crt240p60.nominal_period_us(),
            Some(16_652)
        );
        assert_eq!(
            ResolvedOutputRoute::Crt288p50.nominal_period_us(),
            Some(19_830)
        );
        assert_eq!(
            ResolvedOutputRoute::Crt480p60.nominal_period_us(),
            Some(16_683)
        );
        assert_eq!(
            ResolvedOutputRoute::Crt576p50.nominal_period_us(),
            Some(19_829)
        );
    }

    #[test]
    fn ui_pixel_size_mapping_is_exhaustive_and_eight_aligned() {
        assert_eq!(
            [
                UiPixelSize::Px8,
                UiPixelSize::Px16,
                UiPixelSize::Px24,
                UiPixelSize::Px32,
            ]
            .map(UiPixelSize::pixels),
            [8, 16, 24, 32]
        );
    }

    use mister_magik_fb::framebuffer::format::rgb565_stride_bytes;

    #[derive(Clone, Copy)]
    struct ExpectedDisplayPlan {
        mode: usize,
        output: (u16, u16),
        scan: (u16, u16),
        framebuffer: (usize, usize),
        stride_bytes: usize,
        rendered_pixels: usize,
    }

    #[test]
    fn predefined_modes_follow_the_launcher_resolution_contract() {
        let cases = [
            ExpectedDisplayPlan {
                mode: 0,
                output: (1280, 720),
                scan: (1280, 720),
                framebuffer: (1280, 720),
                stride_bytes: 2560,
                rendered_pixels: 921_600,
            },
            ExpectedDisplayPlan {
                mode: 1,
                output: (1024, 768),
                scan: (1024, 768),
                framebuffer: (1024, 768),
                stride_bytes: 2048,
                rendered_pixels: 786_432,
            },
            ExpectedDisplayPlan {
                mode: 2,
                output: (720, 480),
                scan: (720, 480),
                framebuffer: (720, 480),
                stride_bytes: 1440,
                rendered_pixels: 345_600,
            },
            ExpectedDisplayPlan {
                mode: 3,
                output: (720, 576),
                scan: (720, 576),
                framebuffer: (720, 576),
                stride_bytes: 1440,
                rendered_pixels: 414_720,
            },
            ExpectedDisplayPlan {
                mode: 4,
                output: (1280, 1024),
                scan: (1280, 1024),
                framebuffer: (640, 512),
                stride_bytes: 1280,
                rendered_pixels: 327_680,
            },
            ExpectedDisplayPlan {
                mode: 5,
                output: (800, 600),
                scan: (800, 600),
                framebuffer: (800, 600),
                stride_bytes: 1600,
                rendered_pixels: 480_000,
            },
            ExpectedDisplayPlan {
                mode: 6,
                output: (640, 480),
                scan: (640, 480),
                framebuffer: (640, 480),
                stride_bytes: 1280,
                rendered_pixels: 307_200,
            },
            ExpectedDisplayPlan {
                mode: 7,
                output: (1280, 720),
                scan: (1280, 720),
                framebuffer: (1280, 720),
                stride_bytes: 2560,
                rendered_pixels: 921_600,
            },
            ExpectedDisplayPlan {
                mode: 8,
                output: (1920, 1080),
                scan: (1920, 1080),
                framebuffer: (960, 540),
                stride_bytes: 1920,
                rendered_pixels: 518_400,
            },
            ExpectedDisplayPlan {
                mode: 9,
                output: (1920, 1080),
                scan: (1920, 1080),
                framebuffer: (960, 540),
                stride_bytes: 1920,
                rendered_pixels: 518_400,
            },
            ExpectedDisplayPlan {
                mode: 10,
                output: (1366, 768),
                scan: (1366, 768),
                framebuffer: (683, 384),
                stride_bytes: 1376,
                rendered_pixels: 262_272,
            },
            ExpectedDisplayPlan {
                mode: 11,
                output: (1024, 600),
                scan: (1024, 600),
                framebuffer: (1024, 600),
                stride_bytes: 2048,
                rendered_pixels: 614_400,
            },
            ExpectedDisplayPlan {
                mode: 12,
                output: (1920, 1440),
                scan: (1920, 1440),
                framebuffer: (960, 720),
                stride_bytes: 1920,
                rendered_pixels: 691_200,
            },
            ExpectedDisplayPlan {
                mode: 13,
                output: (2048, 1536),
                scan: (2048, 1536),
                framebuffer: (1024, 768),
                stride_bytes: 2048,
                rendered_pixels: 786_432,
            },
            ExpectedDisplayPlan {
                mode: 14,
                output: (2560, 1440),
                scan: (1280, 1440),
                framebuffer: (1280, 720),
                stride_bytes: 2560,
                rendered_pixels: 921_600,
            },
        ];

        for expected in cases {
            let ini = format!(
                "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode={}\n",
                expected.mode
            );
            let plan = UiDisplayPlan::from_mister_ini_text(&ini).expect("predefined mode");
            assert_eq!(
                (plan.output_w, plan.output_h),
                expected.output,
                "mode {} output",
                expected.mode
            );
            assert_eq!(
                (plan.scan_w, plan.scan_h),
                expected.scan,
                "mode {} scan",
                expected.mode
            );
            assert_eq!(
                (plan.fb_w, plan.fb_h),
                expected.framebuffer,
                "mode {} framebuffer",
                expected.mode
            );
            assert_eq!(
                rgb565_stride_bytes(plan.fb_w),
                expected.stride_bytes,
                "mode {} stride",
                expected.mode
            );
            assert_eq!(
                plan.fb_w * plan.fb_h,
                expected.rendered_pixels,
                "mode {} pixels",
                expected.mode
            );
        }
    }

    #[test]
    fn direct_video_and_custom_modes_follow_the_same_contract() {
        let direct_video_cases = [
            (0, 0, (640, 240), (640, 240), (640, 480), 1280, 153_600),
            (0, 1, (640, 480), (640, 480), (640, 480), 1280, 307_200),
            (1, 0, (640, 288), (640, 288), (640, 288), 1280, 184_320),
            (1, 1, (640, 576), (640, 576), (640, 576), 1280, 368_640),
        ];
        for (pal, scandoubler, scan, framebuffer, render, stride, pixels) in direct_video_cases {
            let ini = format!(
                "[MiSTer]\ndirect_video=1\nmenu_pal={pal}\nforced_scandoubler={scandoubler}\n"
            );
            let plan = UiDisplayPlan::from_mister_ini_text(&ini).expect("direct-video mode");
            assert_eq!((plan.output_w, plan.output_h), scan);
            assert_eq!((plan.scan_w, plan.scan_h), scan);
            assert_eq!((plan.fb_w, plan.fb_h), framebuffer);
            assert_eq!((plan.render_w, plan.render_h), render);
            assert_eq!(rgb565_stride_bytes(plan.fb_w), stride);
            assert_eq!(plan.fb_w * plan.fb_h, pixels);
        }

        let custom = UiDisplayPlan::from_mister_ini_text(
            "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=1920,1200,60\n",
        )
        .expect("custom 1920x1200 mode");
        assert_eq!((custom.output_w, custom.output_h), (1920, 1200));
        assert_eq!((custom.scan_w, custom.scan_h), (1920, 1200));
        assert_eq!((custom.fb_w, custom.fb_h), (960, 600));
        assert_eq!(rgb565_stride_bytes(custom.fb_w), 1920);
        assert_eq!(custom.fb_w * custom.fb_h, 576_000);
    }

    #[test]
    fn production_output_route_matrix_is_explicit_and_complete() {
        let runtime = RuntimeDisplayGeometry::from_video_words(1920, 1080, 1920, 1080);
        let cases = [
            (
                "schema=1&output=hdmi",
                ResolvedOutputRoute::Hdmi,
                (1920, 1080),
                (1920, 1080),
                (960, 540),
                (960, 540),
            ),
            (
                "schema=1&output=crt-240p60",
                ResolvedOutputRoute::Crt240p60,
                (640, 240),
                (640, 240),
                (640, 240),
                (640, 480),
            ),
            (
                "schema=1&output=crt-288p50",
                ResolvedOutputRoute::Crt288p50,
                (640, 288),
                (640, 288),
                (640, 288),
                (640, 288),
            ),
            (
                "schema=1&output=crt-480p60",
                ResolvedOutputRoute::Crt480p60,
                (640, 480),
                (640, 480),
                (640, 480),
                (640, 480),
            ),
            (
                "schema=1&output=crt-576p50",
                ResolvedOutputRoute::Crt576p50,
                (640, 576),
                (640, 576),
                (640, 576),
                (640, 576),
            ),
        ];

        for (settings, route, output, scan, framebuffer, render) in cases {
            let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
                runtime,
                "[Menu]\nvideo_mode=8\n",
                Some(settings),
                None,
            )
            .expect("supported production route");
            assert_eq!(plan.output_route, route, "{settings}");
            assert_eq!((plan.output_w, plan.output_h), output, "{settings}");
            assert_eq!((plan.scan_w, plan.scan_h), scan, "{settings}");
            assert_eq!((plan.fb_w, plan.fb_h), framebuffer, "{settings}");
            assert_eq!((plan.render_w, plan.render_h), render, "{settings}");
            assert_eq!(plan.direct_video, route.is_crt(), "{settings}");
        }
    }

    #[test]
    fn display_plan_halves_hd_modes() {
        let plan = UiDisplayPlan::from_output(1920, 1080, false, "test");
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
        assert_eq!(plan.fb_policy, UiFramebufferSizePolicy::Auto);
        let ui = UiDisplay::for_plan(plan);
        assert_eq!(ui.render_w(), 960);
        assert_eq!(ui.render_h(), 540);
    }

    #[test]
    fn portrait_layout_swaps_only_logical_dimensions() {
        let ui = UiDisplay::for_plan(UiDisplayPlan::from_output(1920, 1080, false, "test"));
        let layout = UiLayoutGeometry::for_display(&ui, ScreenOrientation::MonitorClockwise);

        assert_eq!((layout.logical_w(), layout.logical_h()), (540, 960));
        assert_eq!((layout.composition_w(), layout.composition_h()), (960, 540));
        assert_eq!(
            layout.output_layout().rotation(),
            OutputRotation::CounterClockwise90
        );
        assert_eq!(layout.output_layout().physical_stride(), 960);
        assert_eq!((ui.fb_w(), ui.fb_h()), (960, 540));
        assert_eq!((ui.output_w(), ui.output_h()), (1920, 1080));
        assert_eq!((ui.scan_w(), ui.scan_h()), (1920, 1080));
    }

    #[test]
    fn portrait_pixel_transforms_are_inverse_for_asymmetric_corners() {
        let ui = UiDisplay::for_framebuffer(7, 5);
        for orientation in [
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
        ] {
            let layout = UiLayoutGeometry::for_display(&ui, orientation);
            let logical_corners = [
                (0, 0),
                (layout.logical_w() - 1, 0),
                (0, layout.logical_h() - 1),
                (layout.logical_w() - 1, layout.logical_h() - 1),
                (1, 3),
            ];
            for (x, y) in logical_corners {
                let composition = layout.logical_pixel_to_composition(x, y);
                assert_eq!(
                    layout.composition_pixel_to_logical(composition.0, composition.1),
                    (x, y)
                );
            }
        }
    }

    #[test]
    fn portrait_dirty_rect_mapping_round_trips() {
        let ui = UiDisplay::for_framebuffer(11, 7);
        let logical = DirtyRect {
            x0: 1,
            y0: 2,
            x1: 5,
            y1: 9,
        };
        for orientation in [
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
        ] {
            let layout = UiLayoutGeometry::for_display(&ui, orientation);
            let composition = layout.logical_rect_to_composition(logical);
            assert_eq!(
                layout.composition_rect_to_logical_rect(composition),
                logical
            );
            assert_eq!(composition.width(), logical.rows() as usize);
            assert_eq!(composition.rows() as usize, logical.width());
        }
    }

    #[test]
    fn portrait_rotates_asymmetric_crt_content_insets() {
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
            None,
            "[MiSTer]\ndirect_video=1\nmenu_pal=1\nforced_scandoubler=1\n",
            Some("schema=1&output=crt-576p50"),
            None,
        )
        .expect("CRT 576p route");
        let ui = UiDisplay::for_plan(plan);
        let normal = ui.content_rect();
        let clockwise =
            UiLayoutGeometry::for_display(&ui, ScreenOrientation::MonitorClockwise).content_rect();
        let counterclockwise =
            UiLayoutGeometry::for_display(&ui, ScreenOrientation::MonitorCounterclockwise)
                .content_rect();

        assert_eq!(
            (clockwise.width, clockwise.height),
            (normal.height, normal.width)
        );
        assert_eq!(
            (counterclockwise.width, counterclockwise.height),
            (normal.height, normal.width)
        );
        assert_eq!(clockwise.y, normal.x);
        assert_eq!(counterclockwise.x, normal.y);
    }

    #[test]
    fn framebuffer_size_policy_parses_supported_env_values() {
        assert_eq!(
            UiFramebufferSizePolicy::parse("auto"),
            Some(UiFramebufferSizePolicy::Auto)
        );
        assert_eq!(
            UiFramebufferSizePolicy::parse("960x540"),
            Some(UiFramebufferSizePolicy::Force960x540)
        );
        assert_eq!(
            UiFramebufferSizePolicy::parse("1280x720"),
            Some(UiFramebufferSizePolicy::Force1280x720)
        );
        assert_eq!(UiFramebufferSizePolicy::parse("1920x1080"), None);
        assert_eq!(UiFramebufferSizePolicy::env_name(), "MISTER_UI_FB_SIZE");
    }

    #[test]
    fn forced_1280x720_policy_keeps_1080p_output_and_scan_geometry() {
        let runtime =
            RuntimeDisplayGeometry::from_video_words(1920, 1080, 1920, 1080).expect("runtime");
        let plan = UiDisplayPlan::from_runtime_geometry_with_policy(
            runtime,
            false,
            UiFramebufferSizePolicy::Force1280x720,
        );

        assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
        assert_eq!((plan.scan_w, plan.scan_h), (1920, 1080));
        assert_eq!((plan.fb_w, plan.fb_h), (1280, 720));
        assert_eq!(plan.fb_policy.label(), "forced-1280x720");
        assert!(plan.log_line().contains("fb_policy=forced-1280x720"));
    }

    #[test]
    fn forced_960x540_policy_can_override_native_720p_output() {
        let runtime =
            RuntimeDisplayGeometry::from_video_words(1280, 720, 1280, 720).expect("runtime");
        let plan = UiDisplayPlan::from_runtime_geometry_with_policy(
            runtime,
            false,
            UiFramebufferSizePolicy::Force960x540,
        );

        assert_eq!((plan.output_w, plan.output_h), (1280, 720));
        assert_eq!((plan.scan_w, plan.scan_h), (1280, 720));
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
        assert_eq!(plan.fb_policy.label(), "forced-960x540");
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
        assert_eq!(launcher_framebuffer_size(1366, 768), (683, 384));
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
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
            runtime,
            ini,
            Some("schema=1&output=hdmi"),
            None,
        )
        .expect("plan");

        assert_eq!(plan.source, "runtime-video-info");
        assert_eq!((plan.output_w, plan.output_h), (1280, 720));
        assert_eq!((plan.fb_w, plan.fb_h), (1280, 720));
        assert!(!plan.direct_video);
    }

    #[test]
    fn resolved_crt_modes_override_detected_hdmi_geometry() {
        let runtime = RuntimeDisplayGeometry::from_video_words(640, 480, 640, 480);
        let ini = "[MiSTer]\ndirect_video=2\n[Menu]\nvideo_mode=8\n";
        for (settings, route, scan, framebuffer) in [
            (
                "schema=1&output=crt-240p60",
                ResolvedOutputRoute::Crt240p60,
                (640, 240),
                (640, 240),
            ),
            (
                "schema=1&output=crt-288p50",
                ResolvedOutputRoute::Crt288p50,
                (640, 288),
                (640, 288),
            ),
            (
                "schema=1&output=crt-480p60",
                ResolvedOutputRoute::Crt480p60,
                (640, 480),
                (640, 480),
            ),
            (
                "schema=1&output=crt-576p50",
                ResolvedOutputRoute::Crt576p50,
                (640, 576),
                (640, 576),
            ),
        ] {
            let plan =
                UiDisplayPlan::from_runtime_or_mister_ini_text(runtime, ini, Some(settings), None)
                    .expect("plan");

            assert_eq!(plan.source, "test-runtime-settings-crt");
            assert_eq!((plan.output_w, plan.output_h), scan);
            assert_eq!((plan.scan_w, plan.scan_h), scan);
            assert_eq!((plan.fb_w, plan.fb_h), framebuffer);
            let ui = UiDisplay::for_plan(plan);
            assert_eq!(
                (ui.render_w(), ui.render_h()),
                match route {
                    ResolvedOutputRoute::Crt288p50 => (640, 288),
                    ResolvedOutputRoute::Crt576p50 => (640, 576),
                    _ => (640, 480),
                }
            );
            assert!(plan.direct_video);
            assert_eq!(plan.output_route, route);
        }
    }

    #[test]
    fn crt_ui_metrics_use_native_density_at_every_supported_framebuffer_size() {
        let compact = CrtUiMetrics::for_framebuffer(320, 240);
        assert_eq!(
            (
                compact.grid_x,
                compact.grid_y,
                compact.border_x,
                compact.border_y,
                compact.body_font.pixels(),
                compact.heading_font.pixels(),
                compact.card_title_font.pixels(),
                compact.card_detail_font.pixels(),
                compact.game_row_height,
                compact.header_height,
                compact.footer_height,
            ),
            (4, 4, 1, 1, 8, 16, 16, 8, 24, 48, 24)
        );
        assert_eq!(CrtUiMetrics::for_framebuffer(384, 288), compact);
        assert_eq!(CrtUiMetrics::for_framebuffer(640, 480), compact);
    }

    #[test]
    fn crt_routes_own_safe_content_rects_and_scan_family_metrics() {
        for (route, expected_content, expected_metrics) in [
            (
                ResolvedOutputRoute::Crt240p60,
                CrtContentRect {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                },
                (8, 8, 2, 2, 16, 32, 24, 16, 32, 80, 40),
            ),
            (
                ResolvedOutputRoute::Crt288p50,
                CrtContentRect {
                    x: 0,
                    y: 20,
                    width: 640,
                    height: 255,
                },
                (8, 5, 2, 1, 16, 32, 24, 16, 19, 56, 24),
            ),
            (
                ResolvedOutputRoute::Crt480p60,
                CrtContentRect {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                },
                (4, 4, 1, 1, 8, 16, 16, 8, 32, 48, 24),
            ),
            (
                ResolvedOutputRoute::Crt576p50,
                CrtContentRect {
                    x: 0,
                    y: 0,
                    width: 576,
                    height: 576,
                },
                (4, 5, 1, 1, 8, 16, 16, 8, 39, 56, 29),
            ),
        ] {
            let plan = UiDisplayPlan::from_geometry_with_route(
                route.progressive_geometry().unwrap(),
                route,
                "test-crt-content",
                UiFramebufferSizePolicy::Auto,
            );
            let display = UiDisplay::for_plan(plan);
            let metrics = CrtUiMetrics::for_display(&display);

            assert_eq!(display.content_rect(), expected_content);
            assert_eq!(
                (
                    metrics.grid_x,
                    metrics.grid_y,
                    metrics.border_x,
                    metrics.border_y,
                    metrics.body_font.pixels(),
                    metrics.heading_font.pixels(),
                    metrics.card_title_font.pixels(),
                    metrics.card_detail_font.pixels(),
                    metrics.game_row_height,
                    metrics.header_height,
                    metrics.footer_height,
                ),
                expected_metrics
            );
            assert_eq!(metrics.font_family, CrtFontFamily::PressStart2P);
        }
    }

    #[test]
    fn hdmi_framebuffer_policy_cannot_override_crt_render_geometry() {
        let ini = "[MiSTer]\ndirect_video=1\nmenu_pal=1\nforced_scandoubler=1\n";
        for policy in [
            UiFramebufferSizePolicy::Force960x540,
            UiFramebufferSizePolicy::Force1280x720,
        ] {
            let plan = UiDisplayPlan::from_mister_ini_text_with_policy(ini, policy)
                .expect("576p CRT plan");

            assert_eq!((plan.fb_w, plan.fb_h), (640, 576));
            assert_eq!((plan.render_w, plan.render_h), (640, 576));
            assert_eq!((plan.scan_w, plan.scan_h), (640, 576));
            assert_eq!(plan.fb_policy, UiFramebufferSizePolicy::Auto);
            assert!(plan.log_line().contains("composition_transformed=false"));
        }
    }

    #[test]
    fn ini_geometry_is_fallback_when_detection_fails() {
        let ini = "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=8\n";
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
            None,
            ini,
            Some("schema=1&output=hdmi"),
            None,
        )
        .expect("plan");

        assert_eq!(plan.source, "mister-ini-hdmi-fallback");
        assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
    }

    #[test]
    fn custom_ini_geometry_stays_compatible_as_fallback() {
        let ini = "[MiSTer]\ndirect_video=0\n[Menu]\nvideo_mode=1280,110,40,220,720,5,5,20,74250\n";
        let plan =
            UiDisplayPlan::from_runtime_or_mister_ini_text(None, ini, None, None).expect("plan");

        assert_eq!(plan.source, "mister-ini-hdmi-fallback");
        assert_eq!((plan.output_w, plan.output_h), (1280, 720));
        assert_eq!((plan.fb_w, plan.fb_h), (1280, 720));
    }

    #[test]
    fn malformed_or_unsupported_runtime_settings_stay_hdmi() {
        let runtime = RuntimeDisplayGeometry::from_video_words(1920, 1080, 1920, 1080);
        let ini = "[MiSTer]\ndirect_video=2\n[Menu]\nvideo_mode=8\n";
        for settings in [
            None,
            Some("schema=2&output=crt-240p60"),
            Some("schema=1&output=crt-480i"),
            Some("schema=1&output=crt-240p60&extra=1"),
        ] {
            let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(runtime, ini, settings, None)
                .expect("safe HDMI plan");
            assert_eq!(plan.output_route, ResolvedOutputRoute::Hdmi);
            assert!(!plan.direct_video);
            assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
        }
    }

    #[test]
    fn authoritative_runtime_display_mode_overrides_stale_fpga_geometry() {
        let ini = "[Menu]\nvideo_mode=8\n";
        for (runtime, display, output, framebuffer) in [
            (
                RuntimeDisplayGeometry::from_video_words(1920, 1080, 1920, 1080),
                "schema=1&mode=hdmi-1280x720p60",
                (1280, 720),
                (1280, 720),
            ),
            (
                RuntimeDisplayGeometry::from_video_words(1280, 720, 1280, 720),
                "schema=1&mode=hdmi-1920x1080p60",
                (1920, 1080),
                (960, 540),
            ),
        ] {
            let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
                runtime,
                ini,
                Some("schema=1&output=hdmi"),
                Some(display),
            )
            .expect("plan");
            assert_eq!(plan.source, "test-runtime-display-mode");
            assert_eq!((plan.output_w, plan.output_h), output);
            assert_eq!((plan.fb_w, plan.fb_h), framebuffer);
        }
    }

    #[test]
    fn runtime_display_mode_is_strict_and_preserves_special_scan_geometry() {
        for (mode, output) in [
            ("hdmi-1280x720p60", (1280, 720)),
            ("hdmi-1366x768p60", (1366, 768)),
            ("hdmi-1920x1080p60", (1920, 1080)),
            ("hdmi-1920x1200p60", (1920, 1200)),
            ("hdmi-2048x1536p60", (2048, 1536)),
            ("hdmi-2560x1440p60", (2560, 1440)),
        ] {
            let geometry = runtime_display_geometry_v1(&format!("schema=1&mode={mode}"))
                .expect("supported runtime mode");
            assert_eq!((geometry.output_w, geometry.output_h), output);
        }
        let special = runtime_display_geometry_v1("schema=1&mode=hdmi-2560x1440p60").unwrap();
        assert_eq!((special.output_w, special.output_h), (2560, 1440));
        assert_eq!((special.scan_w, special.scan_h), (1280, 1440));
        for invalid in [
            "schema=2&mode=hdmi-1280x720p60",
            "schema=1&mode=hdmi-1280x720p60&extra=1",
            "schema=1&schema=1&mode=hdmi-1280x720p60",
            "schema=1&mode=unsafe",
            "schema=1&mode=auto",
            "schema=1&mode=custom",
        ] {
            assert!(runtime_display_geometry_v1(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn ui_family_is_selected_from_resolved_route_not_aspect_ratio() {
        for route in [
            ResolvedOutputRoute::Crt240p60,
            ResolvedOutputRoute::Crt288p50,
            ResolvedOutputRoute::Crt480p60,
            ResolvedOutputRoute::Crt576p50,
        ] {
            let plan = UiDisplayPlan::from_geometry_with_route(
                route.progressive_geometry().unwrap(),
                route,
                "test-crt-ui",
                UiFramebufferSizePolicy::Auto,
            );
            assert!(plan.uses_crt_ui(), "{}", route.label());
        }

        let hdmi_4_3 = UiDisplayPlan::from_geometry_with_route(
            VideoModeGeometry::new(2048, 1536),
            ResolvedOutputRoute::Hdmi,
            "test-hdmi-ui",
            UiFramebufferSizePolicy::Auto,
        );
        assert!(!hdmi_4_3.uses_crt_ui());
    }

    #[test]
    fn absent_auto_custom_and_invalid_runtime_display_values_fall_back() {
        let runtime = RuntimeDisplayGeometry::from_video_words(1280, 720, 1280, 720);
        let ini = "[Menu]\nvideo_mode=8\n";
        for display in [
            None,
            Some("schema=1&mode=auto"),
            Some("schema=1&mode=custom"),
            Some("schema=1&mode=unsafe"),
        ] {
            let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
                runtime,
                ini,
                Some("schema=1&output=hdmi"),
                display,
            )
            .expect("runtime fallback");
            assert_eq!(plan.source, "runtime-video-info");
            assert_eq!((plan.output_w, plan.output_h), (1280, 720));
        }
        let plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
            None,
            ini,
            Some("schema=1&output=hdmi"),
            Some("schema=1&mode=auto"),
        )
        .expect("ini fallback");
        assert_eq!(plan.source, "mister-ini-hdmi-fallback");
        assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
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
            "[MiSTer]\ndirect_video=0\nmenu_pal=1\nforced_scandoubler=1\n[Menu]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
        )
        .expect("ntsc plan");
        assert_eq!((ntsc.output_w, ntsc.output_h), (640, 240));
        assert_eq!((ntsc.scan_w, ntsc.scan_h), (640, 240));
        assert_eq!((ntsc.fb_w, ntsc.fb_h), (640, 240));
        assert_eq!((ntsc.render_w, ntsc.render_h), (640, 480));
        assert!(ntsc.direct_video);

        let pal31 = UiDisplayPlan::from_mister_ini_text(
            "[MiSTer]\ndirect_video=0\nmenu_pal=0\nforced_scandoubler=0\n[Menu]\ndirect_video=1\nmenu_pal=1\nforced_scandoubler=1\n",
        )
        .expect("pal plan");
        assert_eq!((pal31.output_w, pal31.output_h), (640, 576));
        assert_eq!((pal31.scan_w, pal31.scan_h), (640, 576));
        assert_eq!((pal31.fb_w, pal31.fb_h), (640, 576));
        assert_eq!((pal31.render_w, pal31.render_h), (640, 576));
        assert!(pal31.direct_video);
    }

    #[test]
    fn fallback_plan_keeps_current_production_geometry() {
        let plan = UiDisplayPlan::fallback_1080p();
        assert_eq!((plan.output_w, plan.output_h), (1920, 1080));
        assert_eq!((plan.fb_w, plan.fb_h), (960, 540));
        assert!(plan.fallback);
    }

    #[test]
    fn crt_game_rows_scale_with_the_supported_framebuffer_profiles() {
        assert_eq!(CrtUiMetrics::for_framebuffer(320, 240).game_row_height, 24);
        assert_eq!(CrtUiMetrics::for_framebuffer(384, 288).game_row_height, 24);
        assert_eq!(CrtUiMetrics::for_framebuffer(640, 480).game_row_height, 24);
    }
}
