// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

/// Timing fields needed to position the HPS framebuffer inside a MiSTer video mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferRouteMode {
    pub hact: u16,
    pub hbp: u16,
    pub vact: u16,
    pub vbp: u16,
}

impl FramebufferRouteMode {
    pub const fn framebuffer_sized(width: u16, height: u16) -> Self {
        Self {
            hact: width,
            hbp: 3,
            vact: height,
            vbp: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferPlacement {
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

impl FramebufferPlacement {
    pub const fn from_mode(mode: FramebufferRouteMode) -> Self {
        Self {
            left: mode.hbp.saturating_sub(3),
            top: mode.vbp.saturating_sub(2),
            width: mode.hact,
            height: mode.vact,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LauncherFramebufferRoute {
    mode: FramebufferRouteMode,
    placement: FramebufferPlacement,
    set_vga_fb: bool,
}

impl LauncherFramebufferRoute {
    pub const fn for_scan(scan_w: u16, scan_h: u16, set_vga_fb: bool) -> Self {
        let mode = ui_fpga_scaled_mode(scan_w, scan_h, set_vga_fb);
        Self {
            mode,
            placement: ui_fpga_placement(mode, set_vga_fb),
            set_vga_fb,
        }
    }

    pub const fn mode(self) -> FramebufferRouteMode {
        self.mode
    }

    pub const fn set_vga_fb(self) -> bool {
        self.set_vga_fb
    }

    pub const fn placement(self) -> FramebufferPlacement {
        self.placement
    }
}

pub const fn ui_fpga_scaled_mode(
    scan_w: u16,
    scan_h: u16,
    direct_video: bool,
) -> FramebufferRouteMode {
    let (hbp, vbp) = if direct_video {
        // Main's standard Menu Direct Video timings. Route programming
        // subtracts the FPGA's fixed 3/2-pixel border from these porches.
        match (scan_w, scan_h) {
            (640, 240 | 288) => (70, 14),
            (640, 480) => (48, 33),
            (640, 576) => (48, 42),
            _ => (3, 2),
        }
    } else {
        (3, 2)
    };
    FramebufferRouteMode {
        hact: scan_w,
        hbp,
        vact: scan_h,
        vbp,
    }
}

const fn ui_fpga_placement(mode: FramebufferRouteMode, direct_video: bool) -> FramebufferPlacement {
    let default = FramebufferPlacement::from_mode(mode);
    if !direct_video {
        return default;
    }
    match (mode.hact, mode.vact) {
        // The 288p capture path expands a full-height destination beyond the
        // visible raster. Inset by eight lines on each edge.
        (640, 288) => FramebufferPlacement {
            top: default.top + 8,
            height: 272,
            ..default
        },
        // The 576p route already has the correct width and vertical scaling,
        // but its porch-derived horizontal displacement clips the right edge.
        (640, 576) => FramebufferPlacement { left: 0, ..default },
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_framebuffer_route_uses_scan_dimensions_and_direct_video_flag() {
        let route = LauncherFramebufferRoute::for_scan(960, 540, true);

        assert_eq!(route.mode().hact, 960);
        assert_eq!(route.mode().vact, 540);
        assert!(route.set_vga_fb());
    }

    #[test]
    fn launcher_route_stays_scan_sized_for_forced_720p_framebuffer() {
        let route = LauncherFramebufferRoute::for_scan(1920, 1080, false);

        assert_eq!(route.mode().hact, 1920);
        assert_eq!(route.mode().vact, 1080);
        assert!(!route.set_vga_fb());
    }

    #[test]
    fn crt_modes_use_main_direct_video_back_porches_and_framebuffer_origins() {
        for (scan_h, hbp, vbp, xoff, yoff) in [
            (240, 70, 14, 67, 12),
            (288, 70, 14, 67, 12),
            (480, 48, 33, 45, 31),
            (576, 48, 42, 45, 40),
        ] {
            let route = LauncherFramebufferRoute::for_scan(640, scan_h, true);
            let mode = route.mode();

            assert_eq!((mode.hact, mode.vact), (640, scan_h));
            assert_eq!((mode.hbp, mode.vbp), (hbp, vbp));
            assert_eq!((mode.hbp as i32 - 3, mode.vbp as i32 - 2), (xoff, yoff));
        }
    }

    #[test]
    fn crt_routes_apply_only_the_observed_288p_and_576p_placement_corrections() {
        let expected = [
            (
                240,
                FramebufferPlacement {
                    left: 67,
                    top: 12,
                    width: 640,
                    height: 240,
                },
            ),
            (
                288,
                FramebufferPlacement {
                    left: 67,
                    top: 20,
                    width: 640,
                    height: 272,
                },
            ),
            (
                480,
                FramebufferPlacement {
                    left: 45,
                    top: 31,
                    width: 640,
                    height: 480,
                },
            ),
            (
                576,
                FramebufferPlacement {
                    left: 0,
                    top: 40,
                    width: 640,
                    height: 576,
                },
            ),
        ];
        for (scan_h, placement) in expected {
            assert_eq!(
                LauncherFramebufferRoute::for_scan(640, scan_h, true).placement(),
                placement
            );
        }
    }

    #[test]
    fn non_direct_video_routes_keep_neutral_back_porches() {
        for scan_h in [240, 288, 480, 576] {
            let route = LauncherFramebufferRoute::for_scan(640, scan_h, false);

            assert_eq!(route.mode().hbp, 3);
            assert_eq!(route.mode().vbp, 2);
        }
    }

    #[test]
    fn unknown_direct_video_geometry_keeps_neutral_back_porches() {
        let route = LauncherFramebufferRoute::for_scan(960, 540, true);

        assert_eq!(route.mode().hbp, 3);
        assert_eq!(route.mode().vbp, 2);
    }
}
