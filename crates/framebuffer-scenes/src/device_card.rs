// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Card-to-system reveal; all text remains at its native raster size.
use crate::Rgb565Pixel;
use crate::card_page::{alpha_of, blend, ease_in_out, ease_out, rounded_span, window_q16};
use crate::navigation::NavigationTransitionRect;
use crate::system_panel::{HDMI_HUB_BANDS, blit, crt_hub_bands};

#[derive(Clone, Debug)]
pub struct RevealImage {
    pub pixels: std::sync::Arc<[u16]>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub reference_height: usize,
    pub integer_scale: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceCardReveal {
    pub region: NavigationTransitionRect,
    pub accent: u16,
    pub crt: bool,
    pub hub: bool,
}
impl DeviceCardReveal {
    pub fn duration_ms(self) -> u32 {
        if self.crt { 900 } else { 1000 }
    }
    pub fn cabinet(crt: bool) -> Self {
        Self {
            region: NavigationTransitionRect {
                x: 30,
                y: 28,
                width: 423,
                height: 462,
            },
            accent: 0xe34b,
            crt,
            hub: true,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_into(
    width: usize,
    height: usize,
    launcher: &[Rgb565Pixel],
    page: &[Rgb565Pixel],
    device: &[Rgb565Pixel],
    backdrop: &[Rgb565Pixel],
    image: Option<&RevealImage>,
    card: NavigationTransitionRect,
    spec: DeviceCardReveal,
    t: u32,
    output: &mut [Rgb565Pixel],
) -> bool {
    let len = width.saturating_mul(height);
    if len == 0
        || width > 960
        || height > 576
        || launcher.len() != len
        || page.len() != len
        || output.len() != len
        || card.width == 0
        || card.height == 0
        || (!spec.crt && device.len() != 483 * 519)
    {
        return false;
    }
    if t == 0 {
        output.copy_from_slice(launcher);
        return true;
    }
    if t >= spec.duration_ms() {
        output.copy_from_slice(page);
        return true;
    }
    if spec.crt {
        crt(
            width, height, launcher, page, backdrop, image, card, spec, t, output,
        );
    } else {
        hdmi(width, height, launcher, page, device, card, spec, t, output);
    }
    true
}
#[allow(clippy::too_many_arguments)]
fn hdmi(
    w: usize,
    h: usize,
    launcher: &[Rgb565Pixel],
    page: &[Rgb565Pixel],
    device: &[Rgb565Pixel],
    card: NavigationTransitionRect,
    spec: DeviceCardReveal,
    t: u32,
    out: &mut [Rgb565Pixel],
) {
    let cx = card.x as f64 + card.width as f64 / 2.0;
    let cy = card.y as f64 + card.height as f64 / 2.0;
    let zmax = 1.15
        * ((cx.max(w as f64 - cx) / (card.width as f64 / 2.0))
            .max(cy.max(h as f64 - cy) / (card.height as f64 / 2.0)));
    let p = window_q16(t, 0, 760);
    let z = (zmax.ln() * ease_in_out(p) as f64 / 65536.0).exp();
    let ww = card.width as f64 * z;
    let hh = card.height as f64 * z;
    let wx = cx - ww / 2.0;
    let wy = cy - hh / 2.0;
    let source_a = 256 - alpha_of(window_q16(t, 40, 240));
    for (dst, src) in out.iter_mut().zip(launcher) {
        *dst = Rgb565Pixel(blend(0, src.0, source_a));
    }
    let face_a = 256 - alpha_of(window_q16(t, 60, 200));
    sample_rect(w, h, launcher, out, card, (wx, wy, ww, hh), face_a, None);
    let pc = ease_in_out(window_q16(t, 80, 760)) as f64 / 65536.0;
    let s0 = card.width as f64 / spec.region.width.max(1) as f64;
    let sc = s0 + (1.0 - s0) * pc;
    let dx = (card.x as f64 - spec.region.x as f64 * s0) * (1.0 - pc) + 490.0 * pc;
    let dy = (card.y as f64 - spec.region.y as f64 * s0) * (1.0 - pc) + 35.0 * pc;
    let inverse = 1.0 / sc;
    let mut device_x = [-1_i32; 960];
    for (x, sx) in device_x.iter_mut().enumerate().take(w) {
        *sx = ((x as f64 + 0.5 - dx) * inverse).floor() as i32;
    }
    for y in 77..500.min(h) {
        let sy = ((y as f64 + 0.5 - dy) * inverse).floor() as i32;
        let span = rounded_span(
            y as i32,
            (wx * 65536.0) as i64,
            (wy * 65536.0) as i64,
            (ww * 65536.0) as i64,
            (hh * 65536.0) as i64,
            (8.0 * z * 65536.0) as i64,
            w,
        );
        if let Some((left, right)) = span {
            for x in left..right {
                let sx = device_x[x];
                if (0..483).contains(&sx) && (0..519).contains(&sy) {
                    let pixel = device[sy as usize * 483 + sx as usize];
                    if pixel.0 != 0 || ((82..402).contains(&sx) && (61..381).contains(&sy)) {
                        out[y * w + x] = pixel;
                    }
                }
            }
        }
    }
    outline(
        w,
        h,
        out,
        (wx, wy, ww, hh),
        8.0 * z,
        spec.accent,
        256_u32.saturating_sub(alpha_of(p * p / 65536).saturating_mul(5) / 4),
        77,
        500,
    );
    blit(
        w,
        h,
        page,
        out,
        (0, w, 0, 77),
        0,
        alpha_of(window_q16(t, 260, 200)),
        None,
    );
    let screenshot = NavigationTransitionRect {
        x: 572,
        y: 96,
        width: 320,
        height: 320,
    };
    blit(
        w,
        h,
        page,
        out,
        (
            screenshot.x as usize,
            screenshot.right() as usize,
            screenshot.y as usize,
            screenshot.bottom() as usize,
        ),
        0,
        alpha_of(window_q16(t, 760, 200)),
        None,
    );
    if spec.hub {
        for (i, b) in HDMI_HUB_BANDS.into_iter().enumerate() {
            let p = ease_out(window_q16(t, 500 + i as u32 * 26, 280));
            blit(
                w,
                h,
                page,
                out,
                b,
                (40 * (65536 - p) / 65536) as isize,
                alpha_of(p),
                None,
            );
        }
    } else {
        for i in 0..11 {
            let p = ease_out(window_q16(t, 500 + i * 26, 280));
            blit(
                w,
                h,
                page,
                out,
                (26, 488, 88 + i as usize * 36, 124 + i as usize * 36),
                (40 * (65536 - p) / 65536) as isize,
                alpha_of(p),
                None,
            );
        }
    }
    blit(
        w,
        h,
        page,
        out,
        (0, w, 500, h),
        0,
        alpha_of(window_q16(t, 640, 220)),
        None,
    );
}
#[allow(clippy::too_many_arguments)]
fn crt(
    w: usize,
    h: usize,
    launcher: &[Rgb565Pixel],
    page: &[Rgb565Pixel],
    backdrop: &[Rgb565Pixel],
    image: Option<&RevealImage>,
    card: NavigationTransitionRect,
    spec: DeviceCardReveal,
    t: u32,
    out: &mut [Rgb565Pixel],
) {
    let p = ease_in_out(window_q16(t, 0, 680)) as f64 / 65536.0;
    let rect = (
        card.x as f64 * (1.0 - p),
        card.y as f64 * (1.0 - p),
        card.width as f64 + (w as f64 - card.width as f64) * p,
        card.height as f64 + (h as f64 - card.height as f64) * p,
    );
    let source_a = 256 - alpha_of(window_q16(t, 100, 260));
    for (dst, src) in out.iter_mut().zip(launcher) {
        *dst = Rgb565Pixel(blend(0, src.0, source_a));
    }
    // Only screenshot pixels scale with the window. UI bands use 1:1 source pixels.
    if let Some(image) = image.filter(|img| {
        img.width > 0
            && img.height > 0
            && img.stride >= img.width
            && img.pixels.len() >= img.stride.saturating_mul(img.height)
    }) {
        image_window(w, h, out, rect, 4.0 * (1.0 - p), image, backdrop, t);
    } else if backdrop.len() == w * h {
        sample_rect(
            w,
            h,
            backdrop,
            out,
            NavigationTransitionRect {
                x: 0,
                y: 0,
                width: w as u16,
                height: h as u16,
            },
            rect,
            256,
            Some((4.0 * (1.0 - p), rect)),
        );
    } else {
        fill_window(w, h, out, rect, 4.0 * (1.0 - p));
    }
    sample_rect(
        w,
        h,
        launcher,
        out,
        card,
        rect,
        256 - alpha_of(window_q16(t, 30, 200)),
        Some((4.0 * (1.0 - p), rect)),
    );
    outline(
        w,
        h,
        out,
        rect,
        4.0 * (1.0 - p),
        spec.accent,
        256_u32.saturating_sub(alpha_of((p * p * 65536.0) as i64).saturating_mul(5) / 4),
        0,
        h,
    );
    let header = h * 35 / 240;
    let footer = h * 210 / 240;
    blit(
        w,
        h,
        page,
        out,
        (0, w, 0, header),
        0,
        alpha_of(window_q16(t, 260, 180)),
        None,
    );
    let count = if spec.hub { 4 } else { 10 };
    for i in 0..count {
        let band = if spec.hub {
            crt_hub_bands(w, h)[i]
        } else {
            (
                38 * w / 640,
                602 * w / 640,
                (35 + i * 16) * h / 240,
                (51 + i * 16) * h / 240,
            )
        };
        let p = ease_out(window_q16(t, 500 + i as u32 * 22, 260));
        blit(
            w,
            h,
            page,
            out,
            band,
            (24 * (65536 - p) / 65536) as isize,
            alpha_of(p),
            Some(backdrop),
        );
    }
    blit(
        w,
        h,
        page,
        out,
        (0, w, footer, h),
        0,
        alpha_of(window_q16(t, 560, 200)),
        None,
    );
}
#[allow(clippy::too_many_arguments)]
fn image_window(
    w: usize,
    h: usize,
    out: &mut [Rgb565Pixel],
    rect: (f64, f64, f64, f64),
    radius: f64,
    image: &RevealImage,
    backdrop: &[Rgb565Pixel],
    t: u32,
) {
    // The native 240p routes use the production 640x480 visual reference and
    // integer 2x artwork. Only the artwork follows the expanding window.
    let retain = 100 - (60 * window_q16(t, 420, 300) / 65536) as u16;
    let red: [u16; 32] = std::array::from_fn(|v| (v as u16 * retain / 100) << 11);
    let green: [u16; 64] = std::array::from_fn(|v| (v as u16 * retain / 100) << 5);
    let blue: [u16; 32] = std::array::from_fn(|v| v as u16 * retain / 100);
    let reference_w = w as f64;
    let reference_h = image.reference_height.max(1) as f64;
    let (image_x, image_y, image_w, image_h) = if image.integer_scale {
        (
            (reference_w - image.width as f64 * 2.0) / 2.0,
            (reference_h - image.height as f64 * 2.0) / 2.0,
            image.width as f64 * 2.0,
            image.height as f64 * 2.0,
        )
    } else {
        let scale = (reference_w / image.width as f64).max(reference_h / image.height as f64);
        let iw = image.width as f64 * scale;
        let ih = image.height as f64 * scale;
        ((reference_w - iw) / 2.0, (reference_h - ih) / 2.0, iw, ih)
    };
    let mut columns = [-1isize; 960];
    for (x, sx) in columns.iter_mut().enumerate().take(w) {
        *sx = (((x as f64 + 0.5 - rect.0) * reference_w / rect.2 - image_x) * image.width as f64
            / image_w)
            .floor() as isize;
    }
    for y in 0..h {
        let sy = (((y as f64 + 0.5 - rect.1) * reference_h / rect.3 - image_y)
            * image.height as f64
            / image_h)
            .floor() as isize;
        if let Some((a, b)) = rounded_span(
            y as i32,
            (rect.0 * 65536.0) as i64,
            (rect.1 * 65536.0) as i64,
            (rect.2 * 65536.0) as i64,
            (rect.3 * 65536.0) as i64,
            (radius * 65536.0) as i64,
            w,
        ) {
            for x in a..b {
                let sx = columns[x];
                let pixel = if sx >= 0
                    && sx < image.width as isize
                    && sy >= 0
                    && sy < image.height as isize
                {
                    let p = image.pixels[sy as usize * image.stride + sx as usize];
                    red[(p >> 11) as usize]
                        | green[((p >> 5) & 63) as usize]
                        | blue[(p & 31) as usize]
                } else {
                    backdrop.get(y * w + x).map_or(0, |pixel| pixel.0)
                };
                out[y * w + x] = Rgb565Pixel(pixel);
            }
        }
    }
}

fn fill_window(w: usize, h: usize, out: &mut [Rgb565Pixel], r: (f64, f64, f64, f64), radius: f64) {
    for y in 0..h {
        if let Some((a, b)) = rounded_span(
            y as i32,
            (r.0 * 65536.0) as i64,
            (r.1 * 65536.0) as i64,
            (r.2 * 65536.0) as i64,
            (r.3 * 65536.0) as i64,
            (radius * 65536.0) as i64,
            w,
        ) {
            out[y * w + a..y * w + b].fill(Rgb565Pixel(0));
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn sample_rect(
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
    out: &mut [Rgb565Pixel],
    s: NavigationTransitionRect,
    r: (f64, f64, f64, f64),
    alpha: u32,
    clip: Option<(f64, (f64, f64, f64, f64))>,
) {
    if alpha == 0 {
        return;
    }
    let inverse_x = s.width as f64 / r.2;
    let inverse_y = s.height as f64 / r.3;
    let mut columns = [0usize; 960];
    for (x, sx) in columns.iter_mut().enumerate().take(w) {
        *sx = s.x as usize
            + (((x as f64 + 0.5 - r.0) * inverse_x).floor().max(0.0) as usize)
                .min(s.width as usize - 1);
    }
    for y in r.1.max(0.0) as usize..(r.1 + r.3).ceil().clamp(0.0, h as f64) as usize {
        let sy = s.y as usize
            + (((y as f64 + 0.5 - r.1) * inverse_y).floor().max(0.0) as usize)
                .min(s.height as usize - 1);
        if sy >= h {
            continue;
        }
        let span = if let Some((radius, c)) = clip {
            rounded_span(
                y as i32,
                (c.0 * 65536.0) as i64,
                (c.1 * 65536.0) as i64,
                (c.2 * 65536.0) as i64,
                (c.3 * 65536.0) as i64,
                (radius * 65536.0) as i64,
                w,
            )
        } else {
            Some((
                r.0.max(0.0) as usize,
                (r.0 + r.2).ceil().clamp(0.0, w as f64) as usize,
            ))
        };
        if let Some((a, b)) = span {
            for x in a..b {
                let sx = columns[x];
                if sx >= w {
                    continue;
                }
                out[y * w + x] = Rgb565Pixel(blend(out[y * w + x].0, src[sy * w + sx].0, alpha));
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn outline(
    w: usize,
    h: usize,
    out: &mut [Rgb565Pixel],
    r: (f64, f64, f64, f64),
    radius: f64,
    accent: u16,
    alpha: u32,
    top: usize,
    bottom: usize,
) {
    if alpha == 0 {
        return;
    }
    for y in top..bottom.min(h) {
        let outer = rounded_span(
            y as i32,
            ((r.0 - 2.0) * 65536.0) as i64,
            ((r.1 - 2.0) * 65536.0) as i64,
            ((r.2 + 4.0) * 65536.0) as i64,
            ((r.3 + 4.0) * 65536.0) as i64,
            ((radius + 2.0) * 65536.0) as i64,
            w,
        );
        if let Some((a, b)) = outer {
            let inner = rounded_span(
                y as i32,
                (r.0 * 65536.0) as i64,
                (r.1 * 65536.0) as i64,
                (r.2 * 65536.0) as i64,
                (r.3 * 65536.0) as i64,
                (radius * 65536.0) as i64,
                w,
            )
            .unwrap_or((b, b));
            for x in (a..inner.0).chain(inner.1..b) {
                out[y * w + x] = Rgb565Pixel(blend(out[y * w + x].0, accent, alpha));
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crt_scrim_uses_original_pixels_and_finishes_at_production_brightness() {
        let w = 640;
        let h = 240;
        let src = vec![Rgb565Pixel(0); w * h];
        let mut page = vec![Rgb565Pixel(0); w * h];
        page[120 * w + 320] = Rgb565Pixel(0x632c);
        let image = RevealImage {
            pixels: vec![0xffff; 320 * 240].into(),
            width: 320,
            height: 240,
            stride: 320,
            reference_height: 480,
            integer_scale: true,
        };
        let card = NavigationTransitionRect {
            x: 38,
            y: 54,
            width: 160,
            height: 112,
        };
        let spec = DeviceCardReveal::cabinet(true);
        let mut out = vec![Rgb565Pixel(0); w * h];
        for (t, expected) in [(420, 0xffff), (720, 0x632c)] {
            assert!(render_into(
                w,
                h,
                &src,
                &page,
                &[],
                &[],
                Some(&image),
                card,
                spec,
                t,
                &mut out
            ));
            assert_eq!(out[120 * w + 320].0, expected, "t={t}");
        }
    }

    #[test]
    fn crt_last_frames_match_the_settled_backdrop() {
        let (w, h) = (640, 240);
        let pixels: Vec<u16> = (0..320 * 240).map(|i| (i * 137) as u16).collect();
        let image = RevealImage {
            pixels: pixels.clone().into(),
            width: 320,
            height: 240,
            stride: 320,
            reference_height: 480,
            integer_scale: true,
        };
        let mut page: Vec<_> = (0..w * h)
            .map(|i| {
                let p = pixels[i / w * 320 + (i % w) / 2];
                Rgb565Pixel(
                    (((p >> 11) * 40 / 100) << 11)
                        | ((((p >> 5) & 63) * 40 / 100) << 5)
                        | ((p & 31) * 40 / 100),
                )
            })
            .collect();
        let backdrop = page.clone();
        page[103 * w + 100] = Rgb565Pixel(0x5b9c);
        let source = vec![Rgb565Pixel(0xffff); w * h];
        let mut out = vec![Rgb565Pixel(0); w * h];
        for t in [850, 867, 883, 899, 900] {
            assert!(render_into(
                w,
                h,
                &source,
                &page,
                &[],
                &backdrop,
                Some(&image),
                NavigationTransitionRect {
                    x: 38,
                    y: 54,
                    width: 160,
                    height: 112
                },
                DeviceCardReveal::cabinet(true),
                t,
                &mut out
            ));
            assert!(
                out == page,
                "t={t}: changed pixels={}",
                out.iter().zip(&page).filter(|(a, b)| a != b).count()
            );
        }
    }

    #[test]
    fn hdmi_black_screen_pixels_occlude_the_fading_card() {
        let (w, h) = (960, 540);
        let source = vec![Rgb565Pixel(0xffff); w * h];
        let page = vec![Rgb565Pixel(0); w * h];
        let device = vec![Rgb565Pixel(0); 483 * 519];
        let mut out = vec![Rgb565Pixel(0); w * h];
        assert!(render_into(
            w,
            h,
            &source,
            &page,
            &device,
            &[],
            None,
            NavigationTransitionRect {
                x: 292,
                y: 158,
                width: 180,
                height: 252
            },
            DeviceCardReveal::cabinet(false),
            120,
            &mut out
        ));
        assert_eq!(out[232 * w + 368], Rgb565Pixel(0));
    }

    #[test]
    fn reveal_uses_the_existing_native_card_slots() {
        let hdmi = crate::launcher::LauncherScene::new(960, 540);
        assert_eq!(
            hdmi.slot_zero(false).rect(),
            NavigationTransitionRect {
                x: 520,
                y: 158,
                width: 180,
                height: 252
            }
        );
        assert_eq!(
            hdmi.slot_zero(true).rect(),
            NavigationTransitionRect {
                x: 292,
                y: 158,
                width: 180,
                height: 252
            }
        );
        assert_eq!(
            crate::launcher::LauncherScene::crt(640, 240)
                .slot_zero(true)
                .rect(),
            NavigationTransitionRect {
                x: 38,
                y: 54,
                width: 160,
                height: 112
            }
        );
    }

    #[test]
    fn reveal_has_exact_endpoints_on_both_profiles() {
        for (w, h, crt) in [(960, 540, false), (640, 240, true)] {
            let src = vec![Rgb565Pixel(0x1234); w * h];
            let dst = vec![Rgb565Pixel(0x5678); w * h];
            let bg = vec![Rgb565Pixel(0x0203); w * h];
            let device = vec![Rgb565Pixel(0x0809); 483 * 519];
            let mut out = vec![Rgb565Pixel(0); w * h];
            let spec = DeviceCardReveal::cabinet(crt);
            let card = if crt {
                NavigationTransitionRect {
                    x: 38,
                    y: 54,
                    width: 160,
                    height: 112,
                }
            } else {
                NavigationTransitionRect {
                    x: 292,
                    y: 158,
                    width: 180,
                    height: 252,
                }
            };
            for t in [0, 1, 230, 460, 680, 760, spec.duration_ms()] {
                assert!(render_into(
                    w, h, &src, &dst, &device, &bg, None, card, spec, t, &mut out
                ));
                if t == 0 {
                    assert!(out == src);
                }
                if t == spec.duration_ms() {
                    assert!(out == dst);
                }
            }
        }
    }
}
