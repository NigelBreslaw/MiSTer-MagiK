// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub const RGB565_BYTES_PER_PIXEL: usize = 2;
pub const RGB565_BITS_PER_PIXEL: u32 = 16;
pub const RGB565_MODE_FORMAT: u16 = 565;
pub const RGB565_ROUTE_RB: bool = true;

pub const FB_FMT_565: u16 = 0b00100;
pub const FB_FMT_RXB: u16 = 0b10000;

pub const fn align16(bytes: usize) -> usize {
    (bytes + 15) & !15
}

pub const fn rgb565_stride_bytes(width: usize) -> usize {
    align16(width * RGB565_BYTES_PER_PIXEL)
}

pub fn rgb565_mode_line(width: usize, height: usize, stride_bytes: usize) -> String {
    let stride_bytes = if stride_bytes == 0 {
        rgb565_stride_bytes(width)
    } else {
        stride_bytes
    };
    format!(
        "{} {} {} {} {}",
        RGB565_MODE_FORMAT,
        if RGB565_ROUTE_RB { 1 } else { 0 },
        width,
        height,
        stride_bytes
    )
}

pub fn restore_mode_line(
    mode_format: u16,
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> String {
    format!(
        "{} {} {} {} {}",
        mode_format,
        if RGB565_ROUTE_RB { 1 } else { 0 },
        width,
        height,
        stride_bytes
    )
}

pub const fn fb_mode_format_from_bits_per_pixel(bits_per_pixel: u32) -> u16 {
    if bits_per_pixel == RGB565_BITS_PER_PIXEL {
        RGB565_MODE_FORMAT
    } else {
        bits_per_pixel as u16
    }
}

pub const fn production_label() -> &'static str {
    "565"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_stride_matches_mister_alignment() {
        assert_eq!(rgb565_stride_bytes(960), 1920);
        assert_eq!(rgb565_stride_bytes(961), 1936);
        assert_eq!(rgb565_stride_bytes(1280), 2560);
    }

    #[test]
    fn rgb565_mode_line_uses_production_contract() {
        assert_eq!(rgb565_mode_line(960, 540, 0), "565 1 960 540 1920");
        assert_eq!(rgb565_mode_line(960, 540, 1920), "565 1 960 540 1920");
        assert_eq!(rgb565_mode_line(1280, 720, 0), "565 1 1280 720 2560");
    }

    #[test]
    fn restore_mode_line_preserves_numeric_framebuffer_state() {
        assert_eq!(restore_mode_line(32, 960, 540, 3840), "32 1 960 540 3840");
    }

    #[test]
    fn fpga_format_bits_match_main_mister_rgb565_route_value() {
        assert_eq!(FB_FMT_565, 0b00100);
        assert_eq!(FB_FMT_RXB, 0b10000);
    }
}
