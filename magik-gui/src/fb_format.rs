#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FramebufferFormat {
    Xrgb8888,
    Rgb565,
}

impl FramebufferFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Xrgb8888 => 4,
            Self::Rgb565 => 2,
        }
    }

    pub const fn mister_mode_format(self) -> u16 {
        match self {
            Self::Xrgb8888 => 8888,
            Self::Rgb565 => 565,
        }
    }

    pub const fn from_bits_per_pixel(bits_per_pixel: u32) -> Self {
        match bits_per_pixel {
            16 => Self::Rgb565,
            _ => Self::Xrgb8888,
        }
    }

    pub const fn fpga_format_bits(self) -> u16 {
        match self {
            Self::Xrgb8888 => FB_FMT_8888,
            Self::Rgb565 => FB_FMT_565,
        }
    }

    pub const fn default_rb(self) -> bool {
        match self {
            Self::Xrgb8888 => true,
            Self::Rgb565 => true,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Xrgb8888 => "8888",
            Self::Rgb565 => "565",
        }
    }

    pub const fn production_default() -> Self {
        Self::Rgb565
    }

    pub const fn is_diagnostic_override(self) -> bool {
        matches!(self, Self::Xrgb8888)
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "8888" | "xrgb8888" | "XRGB8888" => Some(Self::Xrgb8888),
            "565" | "rgb565" | "RGB565" => Some(Self::Rgb565),
            _ => None,
        }
    }

    pub fn from_env() -> Self {
        std::env::var("MISTER_FB_FORMAT")
            .ok()
            .as_deref()
            .and_then(Self::from_label)
            .unwrap_or_else(Self::production_default)
    }

    pub fn rb_from_env(self) -> bool {
        std::env::var("MISTER_FB_RB")
            .ok()
            .and_then(|s| match s.as_str() {
                "0" | "false" | "off" => Some(false),
                "1" | "true" | "on" => Some(true),
                _ => None,
            })
            .unwrap_or_else(|| self.default_rb())
    }

    pub fn stride_bytes(self, width: usize) -> usize {
        align16(width * self.bytes_per_pixel())
    }

    pub fn mode_line(self, width: usize, height: usize, stride_bytes: usize, rb: bool) -> String {
        let stride_bytes = if stride_bytes == 0 {
            self.stride_bytes(width)
        } else {
            stride_bytes
        };
        format!(
            "{} {} {} {} {}",
            self.mister_mode_format(),
            if rb { 1 } else { 0 },
            width,
            height,
            stride_bytes
        )
    }
}

pub const FB_FMT_565: u16 = 0b00100;
pub const FB_FMT_8888: u16 = 0b00110;
pub const FB_FMT_RXB: u16 = 0b10000;

pub const fn align16(bytes: usize) -> usize {
    (bytes + 15) & !15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_matches_mister_alignment() {
        assert_eq!(FramebufferFormat::Xrgb8888.stride_bytes(960), 3840);
        assert_eq!(FramebufferFormat::Rgb565.stride_bytes(960), 1920);
        assert_eq!(FramebufferFormat::Rgb565.stride_bytes(961), 1936);
    }

    #[test]
    fn production_default_is_rgb565() {
        assert_eq!(
            FramebufferFormat::production_default(),
            FramebufferFormat::Rgb565
        );
        assert!(FramebufferFormat::Xrgb8888.is_diagnostic_override());
        assert!(!FramebufferFormat::Rgb565.is_diagnostic_override());
    }

    #[test]
    fn mode_lines_include_format_and_rb() {
        assert_eq!(
            FramebufferFormat::Xrgb8888.mode_line(960, 540, 0, true),
            "8888 1 960 540 3840"
        );
        assert_eq!(
            FramebufferFormat::Rgb565.mode_line(960, 540, 0, true),
            "565 1 960 540 1920"
        );
    }

    #[test]
    fn mode_lines_preserve_reported_stride() {
        assert_eq!(
            FramebufferFormat::Xrgb8888.mode_line(960, 540, 3840, true),
            "8888 1 960 540 3840"
        );
        assert_eq!(
            FramebufferFormat::Rgb565.mode_line(960, 540, 1920, true),
            "565 1 960 540 1920"
        );
    }

    #[test]
    fn format_can_be_restored_from_framebuffer_bpp() {
        assert_eq!(
            FramebufferFormat::from_bits_per_pixel(16).mode_line(960, 540, 1920, true),
            "565 1 960 540 1920"
        );
        assert_eq!(
            FramebufferFormat::from_bits_per_pixel(32).mode_line(960, 540, 3840, true),
            "8888 1 960 540 3840"
        );
    }
}
