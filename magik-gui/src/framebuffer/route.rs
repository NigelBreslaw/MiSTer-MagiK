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
pub struct LauncherFramebufferRoute {
    mode: FramebufferRouteMode,
    set_vga_fb: bool,
}

impl LauncherFramebufferRoute {
    pub const fn for_scan(scan_w: u16, scan_h: u16, set_vga_fb: bool) -> Self {
        Self {
            mode: ui_fpga_scaled_mode(scan_w, scan_h),
            set_vga_fb,
        }
    }

    pub const fn mode(self) -> FramebufferRouteMode {
        self.mode
    }

    pub const fn set_vga_fb(self) -> bool {
        self.set_vga_fb
    }
}

pub const fn ui_fpga_scaled_mode(scan_w: u16, scan_h: u16) -> FramebufferRouteMode {
    FramebufferRouteMode {
        hact: scan_w,
        hbp: 3,
        vact: scan_h,
        vbp: 2,
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
}
