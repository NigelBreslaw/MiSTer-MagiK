use crate::fb::FbInfo;
use crate::fpga::{FbParams, Fpga, VideoInfo};
use crate::ui_display::UiDisplay;

#[derive(Clone, Copy, Debug)]
pub struct DisplayConfig {
    pub fb: FbInfo,
    pub video: VideoInfo,
    pub fpga_fb: FbParams,
    pub render_w: usize,
    pub render_h: usize,
    pub fb_scale: usize,
}

impl DisplayConfig {
    pub fn detect(f: &mut Fpga, fb: FbInfo, ui: &UiDisplay) -> Self {
        Self {
            fb,
            video: f.read_video_info(),
            fpga_fb: f.read_fb_params(),
            render_w: ui.render_w(),
            render_h: ui.render_h(),
            fb_scale: ui.fb_scale(),
        }
    }

    pub fn log_line(self) -> String {
        format!(
            "display-config: {}; {}; {}; render={}x{} fb_scale={}",
            self.fb.log_line(),
            self.video.log_line(),
            self.fpga_fb.log_line(),
            self.render_w,
            self.render_h,
            self.fb_scale
        )
    }

    pub fn boot_analytics_detail(self) -> String {
        format!(
            "fb_visible={}x{} fb_virtual={}x{} fb_stride={} fb_bpp={} uio_vres={}x{} uio_pixrep={} uio_de={}x{} uio_fb={}x{} uio_fb_enabled={} render={}x{} fb_scale={}",
            self.fb.visible_w,
            self.fb.visible_h,
            self.fb.virtual_w,
            self.fb.virtual_h,
            self.fb.stride_bytes,
            self.fb.bits_per_pixel,
            self.video.width,
            self.video.height,
            self.video.pixrep,
            self.video.de_h,
            self.video.de_v,
            self.fpga_fb.fb_width,
            self.fpga_fb.fb_height,
            self.fpga_fb.fb_enabled,
            self.render_w,
            self.render_h,
            self.fb_scale
        )
    }
}
