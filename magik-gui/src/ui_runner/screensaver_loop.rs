use super::*;
use crate::preview_worker;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScreensaverMode {
    AttractWall,
    MvsCarousel,
    SuperScalerFlyby,
    StarfieldCabinets,
    ScreenshotRain,
    TilemapMuseum,
    RasterGallery,
    KefrensScreenshotBars,
    PreviewPlasmaCollage,
    PhosphorGrid,
    WarpTunnel,
    Mode7Floor,
    ScannerContactSheet,
    SpriteMultiplexParade,
    CabinetMarquee,
    RandomAccessLoader,
    ColorClashGallery,
    IdleMegademo,
}

impl ScreensaverMode {
    const ALL: [Self; 18] = [
        Self::AttractWall,
        Self::MvsCarousel,
        Self::SuperScalerFlyby,
        Self::StarfieldCabinets,
        Self::ScreenshotRain,
        Self::TilemapMuseum,
        Self::RasterGallery,
        Self::KefrensScreenshotBars,
        Self::PreviewPlasmaCollage,
        Self::PhosphorGrid,
        Self::WarpTunnel,
        Self::Mode7Floor,
        Self::ScannerContactSheet,
        Self::SpriteMultiplexParade,
        Self::CabinetMarquee,
        Self::RandomAccessLoader,
        Self::ColorClashGallery,
        Self::IdleMegademo,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::AttractWall => "attract-wall",
            Self::MvsCarousel => "mvs-carousel",
            Self::SuperScalerFlyby => "super-scaler-flyby",
            Self::StarfieldCabinets => "starfield-cabinets",
            Self::ScreenshotRain => "screenshot-rain",
            Self::TilemapMuseum => "tilemap-museum",
            Self::RasterGallery => "raster-gallery",
            Self::KefrensScreenshotBars => "kefrens-screenshot-bars",
            Self::PreviewPlasmaCollage => "preview-plasma-collage",
            Self::PhosphorGrid => "phosphor-grid",
            Self::WarpTunnel => "warp-tunnel",
            Self::Mode7Floor => "mode7-floor",
            Self::ScannerContactSheet => "scanner-contact-sheet",
            Self::SpriteMultiplexParade => "sprite-multiplex-parade",
            Self::CabinetMarquee => "cabinet-marquee",
            Self::RandomAccessLoader => "random-access-loader",
            Self::ColorClashGallery => "color-clash-gallery",
            Self::IdleMegademo => "idle-megademo",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        let value = value.to_ascii_lowercase().replace('_', "-");
        Self::ALL.iter().copied().find(|mode| mode.label() == value)
    }
}

struct ScreensaverConfig {
    modes: Vec<ScreensaverMode>,
    segment: Duration,
    cache_cap: usize,
    trace: Option<File>,
}

impl ScreensaverConfig {
    fn from_env() -> Self {
        let spec = std::env::var("MISTER_SCREENSAVER").unwrap_or_else(|_| "mega".into());
        let modes = if matches!(
            spec.trim().to_ascii_lowercase().as_str(),
            "" | "mega" | "all" | "demo"
        ) {
            ScreensaverMode::ALL.to_vec()
        } else {
            let mut modes = Vec::new();
            for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(mode) = ScreensaverMode::parse(part) {
                    modes.push(mode);
                } else {
                    eprintln!("screensaver: unknown mode {part:?}");
                }
            }
            if modes.is_empty() {
                vec![ScreensaverMode::AttractWall]
            } else {
                modes
            }
        };
        let segment_secs = std::env::var("MISTER_SCREENSAVER_SEGMENT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(20)
            .max(1);
        let cache_cap = std::env::var("MISTER_SCREENSAVER_CACHE_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(64)
            .clamp(1, 512);
        let trace = std::env::var("MISTER_SCREENSAVER_TRACE")
            .ok()
            .and_then(|path| {
                let mut f = File::create(&path)
                    .map_err(|e| eprintln!("screensaver trace: create {path} failed: {e}"))
                    .ok()?;
                f.write_all(b"frame\telapsed_us\tmode\timage_count\tdraw_us\tvsync_us\tfb_present_us\twall_us\tvsync_source\tvsync_period_us\tvsync_miss_streak\n")
                    .ok()?;
                println!("screensaver_trace={path}");
                Some(f)
            });
        Self {
            modes,
            segment: Duration::from_secs(segment_secs),
            cache_cap,
            trace,
        }
    }

    fn mode_at(&self, elapsed: Duration) -> ScreensaverMode {
        let idx = ((elapsed.as_micros() / self.segment.as_micros().max(1)) as usize)
            % self.modes.len().max(1);
        self.modes
            .get(idx)
            .copied()
            .unwrap_or(ScreensaverMode::AttractWall)
    }
}

#[derive(Clone)]
struct SaverImage {
    pixels: Vec<Rgb565Pixel>,
    w: usize,
    h: usize,
    stride: usize,
}

pub(super) fn run_screensaver_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    fb_format: FramebufferFormat,
) {
    let mut cfg = ScreensaverConfig::from_env();
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let images = load_screensaver_images(&arcade_root, cfg.cache_cap);
    println!(
        "screensaver modes={} segment_secs={} cache_cap={} images={}",
        cfg.modes
            .iter()
            .map(|mode| mode.label())
            .collect::<Vec<_>>()
            .join(","),
        cfg.segment.as_secs(),
        cfg.cache_cap,
        images.len()
    );

    let mut backbuffer = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];
    let mut xrgb_scratch =
        (fb_format == FramebufferFormat::Xrgb8888).then(|| vec![Pixel(0); backbuffer.len()]);
    let mut pacer = VsyncPacer::from_env();
    let start = Instant::now();
    let mut frame = 0_u64;
    loop {
        let frame_start = Instant::now();
        let elapsed = start.elapsed();
        if secs > 0 && elapsed >= Duration::from_secs(secs) {
            break;
        }
        let mode = cfg.mode_at(elapsed);
        let draw_start = Instant::now();
        render_screensaver_frame(
            &mut backbuffer,
            ui.render_w(),
            ui.render_h(),
            &images,
            mode,
            frame,
        );
        let draw_us = draw_start.elapsed().as_micros() as u64;
        let vsync = pacer.wait();
        let present_start = Instant::now();
        match fb_format {
            FramebufferFormat::Rgb565 => disp.copy_rows_565(&backbuffer, 0, ui.render_h()),
            FramebufferFormat::Xrgb8888 => {
                let scratch = xrgb_scratch.as_mut().expect("xrgb scratch");
                for (dst, src) in scratch.iter_mut().zip(backbuffer.iter().copied()) {
                    *dst = rgb565_to_pixel(src);
                }
                disp.copy_rows(scratch, 0, ui.render_h());
            }
        }
        let present_us = present_start.elapsed().as_micros() as u64;
        let wall_us = frame_start.elapsed().as_micros() as u64;
        if let Some(trace) = cfg.trace.as_mut() {
            let _ = writeln!(
                trace,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                frame,
                elapsed.as_micros(),
                mode.label(),
                images.len(),
                draw_us,
                vsync.wait_us,
                present_us,
                wall_us,
                vsync.source.label(),
                vsync.period_us,
                vsync.miss_streak
            );
        }
        frame = frame.wrapping_add(1);
    }
}

fn load_screensaver_images(root: &str, cap: usize) -> Vec<SaverImage> {
    let resize = preview_worker::PreviewResizeSpec::from_env();
    let mut paths = Vec::new();
    if let Ok(loaded) = library_db::load_arcade_catalog_from_sqlite(root) {
        paths.extend(
            loaded
                .catalog
                .games
                .iter()
                .filter(|game| game.has_image && !game.image_path.is_empty())
                .map(|game| game.image_path.clone())
                .take(cap * 4),
        );
    }
    let mut images = Vec::new();
    for path in paths {
        if images.len() >= cap {
            break;
        }
        let cache = preview_worker::raw565_preview_cache_path(&path, resize);
        if let Ok(image) = read_raw565_image(&cache) {
            images.push(image);
        }
    }
    images
}

fn read_raw565_image(path: &Path) -> Result<SaverImage, String> {
    let mut data = Vec::new();
    File::open(path)
        .and_then(|mut f| f.read_to_end(&mut data))
        .map_err(|e| format!("{path:?}: {e}"))?;
    if data.len() < 20 || &data[..8] != b"MM56501\0" {
        return Err(format!("{path:?}: bad raw565 header"));
    }
    let w = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
    let stride_bytes = u32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;
    if w == 0 || h == 0 || stride_bytes < w * 2 || stride_bytes % 16 != 0 {
        return Err(format!("{path:?}: bad raw565 geometry"));
    }
    let expected = 20 + stride_bytes * h;
    if data.len() != expected {
        return Err(format!("{path:?}: raw565 length mismatch"));
    }
    let mut pixels = Vec::with_capacity(stride_bytes / 2 * h);
    for chunk in data[20..].chunks_exact(2) {
        pixels.push(Rgb565Pixel(u16::from_le_bytes([chunk[0], chunk[1]])));
    }
    Ok(SaverImage {
        pixels,
        w,
        h,
        stride: stride_bytes / 2,
    })
}

fn render_screensaver_frame(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    mode: ScreensaverMode,
    frame: u64,
) {
    clear(dst, color565(2, 4, 10));
    match mode {
        ScreensaverMode::AttractWall => render_contact_sheet(dst, w, h, images, frame, 6, 4, false),
        ScreensaverMode::MvsCarousel => render_carousel(dst, w, h, images, frame),
        ScreensaverMode::SuperScalerFlyby => render_flyby(dst, w, h, images, frame),
        ScreensaverMode::StarfieldCabinets => {
            render_starfield(dst, w, h, frame);
            render_contact_sheet(dst, w, h, images, frame / 2, 5, 3, true);
        }
        ScreensaverMode::ScreenshotRain => render_rain(dst, w, h, images, frame),
        ScreensaverMode::TilemapMuseum => render_tilemap(dst, w, h, images, frame),
        ScreensaverMode::RasterGallery => render_raster_gallery(dst, w, h, images, frame),
        ScreensaverMode::KefrensScreenshotBars => render_kefrens(dst, w, h, images, frame),
        ScreensaverMode::PreviewPlasmaCollage => {
            render_plasma(dst, w, h, frame);
            render_contact_sheet(dst, w, h, images, frame / 3, 4, 3, true);
        }
        ScreensaverMode::PhosphorGrid => render_phosphor_grid(dst, w, h, images, frame),
        ScreensaverMode::WarpTunnel => render_warp(dst, w, h, images, frame),
        ScreensaverMode::Mode7Floor => render_mode7(dst, w, h, images, frame),
        ScreensaverMode::ScannerContactSheet => render_scanner(dst, w, h, images, frame),
        ScreensaverMode::SpriteMultiplexParade => render_parade(dst, w, h, images, frame),
        ScreensaverMode::CabinetMarquee => render_marquee(dst, w, h, images, frame),
        ScreensaverMode::RandomAccessLoader => render_random_loader(dst, w, h, images, frame),
        ScreensaverMode::ColorClashGallery => render_color_clash(dst, w, h, images, frame),
        ScreensaverMode::IdleMegademo => {
            let sub =
                ScreensaverMode::ALL[((frame / 240) as usize) % (ScreensaverMode::ALL.len() - 1)];
            render_screensaver_frame(dst, w, h, images, sub, frame);
        }
    }
}

fn color565(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    <Rgb565Pixel as TargetPixel>::from_rgb(r, g, b)
}

fn clear(dst: &mut [Rgb565Pixel], color: Rgb565Pixel) {
    dst.fill(color);
}

fn sample_image(img: &SaverImage, x: usize, y: usize) -> Rgb565Pixel {
    img.pixels[(y.min(img.h - 1)) * img.stride + x.min(img.w - 1)]
}

fn blit_scaled(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    img: &SaverImage,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
) {
    if out_w == 0 || out_h == 0 {
        return;
    }
    for yy in 0..out_h {
        let dy = y + yy as isize;
        if dy < 0 || dy >= screen_h as isize {
            continue;
        }
        let sy = yy * img.h / out_h;
        for xx in 0..out_w {
            let dx = x + xx as isize;
            if dx < 0 || dx >= screen_w as isize {
                continue;
            }
            let sx = xx * img.w / out_w;
            let mut px = sample_image(img, sx, sy);
            if tint < 255 {
                px = blend_565(color565(0, 0, 18), px, tint);
            }
            dst[dy as usize * screen_w + dx as usize] = px;
        }
    }
}

fn image_at(images: &[SaverImage], idx: usize) -> Option<&SaverImage> {
    if images.is_empty() {
        None
    } else {
        images.get(idx % images.len())
    }
}

fn render_contact_sheet(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
    cols: usize,
    rows: usize,
    drift: bool,
) {
    let cell_w = w / cols.max(1);
    let cell_h = h / rows.max(1);
    let start = (frame / 90) as usize;
    for row in 0..rows {
        for col in 0..cols {
            if let Some(img) = image_at(images, start + row * cols + col) {
                let ox = if drift {
                    ((frame as usize + row * 13) & 31) as isize - 16
                } else {
                    0
                };
                let x = (col * cell_w + 8) as isize + ox;
                let y = (row * cell_h + 8) as isize;
                blit_scaled(
                    dst,
                    w,
                    h,
                    img,
                    x,
                    y,
                    cell_w.saturating_sub(16),
                    cell_h.saturating_sub(16),
                    230,
                );
            }
        }
    }
}

fn render_carousel(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    for i in 0..7 {
        if let Some(img) = image_at(images, i + frame as usize / 120) {
            let phase = ((frame as i64 * 3 + i as i64 * 91) % 640) - 320;
            let depth = 180 + ((phase.unsigned_abs() as usize * 220) / 320);
            let out_w = depth.min(360);
            let out_h = out_w * 3 / 4;
            let x = w as isize / 2 + phase as isize - out_w as isize / 2;
            let y = h as isize / 2 - out_h as isize / 2 + ((i as isize - 3).abs() * 8);
            blit_scaled(dst, w, h, img, x, y, out_w, out_h, 255);
        }
    }
}

fn render_flyby(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_starfield(dst, w, h, frame);
    for i in 0..5 {
        if let Some(img) = image_at(images, i + frame as usize / 100) {
            let t = ((frame as usize * 5 + i * 97) % 360).max(1);
            let out_w = 48 + t;
            let out_h = out_w * 3 / 4;
            let x = (w / 2 + (i * 173 + frame as usize * 2) % w) as isize - out_w as isize / 2;
            let y = (h / 2 + (i * 71 + frame as usize) % (h / 2)) as isize - out_h as isize / 2;
            blit_scaled(dst, w, h, img, x, y, out_w, out_h, 220);
        }
    }
}

fn render_starfield(dst: &mut [Rgb565Pixel], w: usize, h: usize, frame: u64) {
    clear(dst, color565(0, 0, 10));
    for i in 0..420 {
        let z = ((i * 17 + frame as usize * 3) % 255).max(1);
        let sx = ((i * 97) % w) as isize - w as isize / 2;
        let sy = ((i * 53) % h) as isize - h as isize / 2;
        let x = w as isize / 2 + sx * 255 / z as isize;
        let y = h as isize / 2 + sy * 255 / z as isize;
        if x >= 0 && y >= 0 && x < w as isize && y < h as isize {
            dst[y as usize * w + x as usize] = color565(80, 220, 255);
        }
    }
}

fn render_rain(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_plasma(dst, w, h, frame / 2);
    for i in 0..18 {
        if let Some(img) = image_at(images, i + frame as usize / 75) {
            let x = ((i * 59 + frame as usize * 2) % (w + 180)) as isize - 90;
            let y = ((i * 113 + frame as usize * (2 + i % 3)) % (h + 150)) as isize - 120;
            blit_scaled(dst, w, h, img, x, y, 112, 84, 210);
        }
    }
}

fn render_tilemap(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_contact_sheet(dst, w, h, images, frame / 2, 8, 5, true);
}

fn render_raster_gallery(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    for y in 0..h {
        let c = (((y + frame as usize) & 63) * 3) as u8;
        for x in 0..w {
            dst[y * w + x] = color565(c / 3, c, 80);
        }
    }
    render_contact_sheet(dst, w, h, images, frame, 5, 3, true);
}

fn render_kefrens(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    for y in 0..h {
        let shift = triangle_wave_u8(y / 3, frame as u8) as isize - 128;
        if let Some(img) = image_at(images, y / 36 + frame as usize / 120) {
            blit_scaled(dst, w, h, img, shift - 80, y as isize, w + 160, 2, 240);
        }
    }
}

fn render_plasma(dst: &mut [Rgb565Pixel], w: usize, h: usize, frame: u64) {
    for y in 0..h {
        for x in 0..w {
            let p = plasma_gate(x, y, frame as u8);
            dst[y * w + x] = color565(p / 5, p.saturating_add(40), 180u8.saturating_sub(p / 3));
        }
    }
}

fn render_phosphor_grid(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    render_contact_sheet(dst, w, h, images, frame / 4, 6, 4, false);
    for y in (0..h).step_by(3) {
        for x in 0..w {
            dst[y * w + x] = darken_565(dst[y * w + x]);
        }
    }
}

fn render_warp(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_starfield(dst, w, h, frame);
    if let Some(img) = image_at(images, frame as usize / 120) {
        for i in 0..10 {
            let size = 60 + i * 36 + (frame as usize % 36);
            blit_scaled(
                dst,
                w,
                h,
                img,
                w as isize / 2 - size as isize / 2,
                h as isize / 2 - size as isize / 3,
                size,
                size * 2 / 3,
                120,
            );
        }
    }
}

fn render_mode7(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    clear(dst, color565(4, 8, 22));
    if let Some(img) = image_at(images, frame as usize / 150) {
        for y in h / 2..h {
            let depth = (y - h / 2 + 1) * 2;
            let span = (w * 80 / depth).max(1);
            for x in 0..w {
                let sx = (x * span / w + frame as usize * 2) % img.w;
                let sy = ((depth + frame as usize) / 3) % img.h;
                dst[y * w + x] = sample_image(img, sx, sy);
            }
        }
    }
}

fn render_scanner(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_contact_sheet(dst, w, h, images, frame / 5, 7, 4, false);
    let scan_y = (frame as usize * 5) % h;
    for y in scan_y.saturating_sub(3)..(scan_y + 4).min(h) {
        for x in 0..w {
            dst[y * w + x] = brighten_565(dst[y * w + x]);
        }
    }
}

fn render_parade(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_starfield(dst, w, h, frame / 2);
    for i in 0..14 {
        if let Some(img) = image_at(images, i + frame as usize / 100) {
            let x = ((frame as usize * (2 + i % 4) + i * 83) % (w + 150)) as isize - 90;
            let y = 40 + (i * 31) % (h - 120);
            blit_scaled(dst, w, h, img, x, y as isize, 96, 72, 230);
        }
    }
}

fn render_marquee(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_plasma(dst, w, h, frame / 3);
    for i in 0..8 {
        if let Some(img) = image_at(images, i + frame as usize / 100) {
            blit_scaled(
                dst,
                w,
                h,
                img,
                (i * 132) as isize - (frame as isize % 132),
                52,
                124,
                92,
                255,
            );
            blit_scaled(
                dst,
                w,
                h,
                img,
                ((7 - i) * 132) as isize + (frame as isize % 132) - 80,
                394,
                124,
                92,
                210,
            );
        }
    }
}

fn render_random_loader(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let tiles_x = 12;
    let tiles_y = 7;
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let idx = tx + ty * tiles_x + frame as usize / 30;
            let color = if hash2_u8(tx + idx, ty) > (frame as u8) {
                color565(0, 18, 30)
            } else {
                color565(20, 180, 210)
            };
            for y in ty * h / tiles_y..(ty + 1) * h / tiles_y {
                for x in tx * w / tiles_x..(tx + 1) * w / tiles_x {
                    dst[y * w + x] = color;
                }
            }
        }
    }
    render_contact_sheet(dst, w, h, images, frame, 4, 3, true);
}

fn render_color_clash(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    render_contact_sheet(dst, w, h, images, frame / 3, 6, 4, false);
    for y in 0..h {
        for x in 0..w {
            if ((x / 16 + y / 16 + frame as usize / 8) & 1) == 0 {
                dst[y * w + x] = brighten_565(dst[y * w + x]);
            }
        }
    }
}
