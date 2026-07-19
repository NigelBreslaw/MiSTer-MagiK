// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::preview_worker;
use std::fs::File;
use std::io::Write;

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
    RadialStarfield,
    IdleMegademo,
}

impl ScreensaverMode {
    const ALL: [Self; 19] = [
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
        Self::RadialStarfield,
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
            Self::RadialStarfield => "radial-starfield",
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
                    crate::ui_errln!("screensaver: unknown mode {part:?}");
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
            .unwrap_or(256)
            .clamp(1, 512);
        let trace = std::env::var("MISTER_SCREENSAVER_TRACE")
            .ok()
            .and_then(|path| {
                let mut f = File::create(&path)
                    .map_err(|e| crate::ui_errln!("screensaver trace: create {path} failed: {e}"))
                    .ok()?;
                f.write_all(b"frame\telapsed_us\tmode\timage_count\tdraw_us\tvsync_us\tfb_present_us\twall_us\tvsync_source\tvsync_period_us\tvsync_miss_streak\n")
                    .ok()?;
                crate::ui_logln!("screensaver_trace={path}");
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

struct ScreensaverRenderState {
    parade: ParadeState,
    phosphor_grid: Vec<Rgb565Pixel>,
    phosphor_grid_page: usize,
    phosphor_grid_valid: bool,
    random_loader: Vec<Rgb565Pixel>,
    random_loader_page: usize,
    random_loader_valid: bool,
    tilemap_normal: Vec<Rgb565Pixel>,
    tilemap_bright: Vec<Rgb565Pixel>,
    tilemap_page: usize,
    tilemap_valid: bool,
    attract_wall_base: Vec<Rgb565Pixel>,
    attract_wall_next: Vec<Rgb565Pixel>,
    attract_wall_page: usize,
    attract_wall_valid: bool,
    color_clash_contact: Vec<Rgb565Pixel>,
    color_clash_contact_start: usize,
    color_clash_contact_valid: bool,
    scanner_contact: Vec<Rgb565Pixel>,
    scanner_contact_start: usize,
    scanner_contact_valid: bool,
    starfield_contact: Vec<Rgb565Pixel>,
    starfield_contact_start: usize,
    starfield_contact_valid: bool,
}

impl ScreensaverRenderState {
    fn new(w: usize, h: usize) -> Self {
        Self {
            parade: ParadeState::new(random_seed()),
            phosphor_grid: vec![Rgb565Pixel(0); w * h],
            phosphor_grid_page: usize::MAX,
            phosphor_grid_valid: false,
            random_loader: vec![Rgb565Pixel(0); w * h],
            random_loader_page: usize::MAX,
            random_loader_valid: false,
            tilemap_normal: vec![Rgb565Pixel(0); w * h],
            tilemap_bright: vec![Rgb565Pixel(0); w * h],
            tilemap_page: usize::MAX,
            tilemap_valid: false,
            attract_wall_base: vec![Rgb565Pixel(0); w * h],
            attract_wall_next: vec![Rgb565Pixel(0); w * h],
            attract_wall_page: usize::MAX,
            attract_wall_valid: false,
            color_clash_contact: vec![Rgb565Pixel(0); w * h],
            color_clash_contact_start: usize::MAX,
            color_clash_contact_valid: false,
            scanner_contact: vec![Rgb565Pixel(0); w * h],
            scanner_contact_start: usize::MAX,
            scanner_contact_valid: false,
            starfield_contact: vec![Rgb565Pixel(0); w * h],
            starfield_contact_start: usize::MAX,
            starfield_contact_valid: false,
        }
    }
}

pub(in crate::ui_runner) fn run_screensaver_loop(
    secs: u64,
    ui: &UiDisplay,
    hardware: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
) {
    let mut cfg = ScreensaverConfig::from_env();
    let images = load_screensaver_images(cfg.cache_cap);
    crate::ui_logln!(
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
    let mut render_state = ScreensaverRenderState::new(ui.render_w(), ui.render_h());
    let mut presenter = match FpgaVblankLatchHiddenPresenter::open(ui) {
        Ok(presenter) => presenter,
        Err(failure) => {
            crate::ui_errln!(
                "screensaver_latch_failure state={} stage={} reason={} detail={}",
                failure.state.code(),
                failure.stage.code(),
                failure.reason_code(),
                failure.detail.replace(['\t', '\n', '\r'], " ")
            );
            return;
        }
    };
    let full_damage = DirtyRectList::from_one(DirtyRect {
        x0: 0,
        y0: 0,
        x1: ui.render_w(),
        y1: ui.render_h(),
    });
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
            &mut render_state,
            ui.render_w(),
            ui.render_h(),
            &images,
            mode,
            frame,
        );
        let draw_us = draw_start.elapsed().as_micros() as u64;
        let present_start = Instant::now();
        let frame_plan = LauncherFramePlan::new(full_damage, None, None, None, None);
        let stats = match presenter.present_cached_full_frame(
            CachedFrameView::new(&backbuffer, ui.render_w(), ui.render_h()),
            frame_plan,
            hardware,
            display_session,
            |_hidden, _plan| Ok(()),
        ) {
            Ok(stats) => stats,
            Err(failure) => {
                crate::ui_errln!(
                    "screensaver_latch_failure state={} stage={} reason={} detail={}",
                    failure.state.code(),
                    failure.stage.code(),
                    failure.reason_code(),
                    failure.detail.replace(['\t', '\n', '\r'], " ")
                );
                break;
            }
        };
        if let Some(scale) = mister_magik_fb::framebuffer::stream::configured_latch_scale(true) {
            let committed = presenter.committed_frame_view(stats.buffer_index);
            let _ = mister_magik_fb::framebuffer::stream::publish_latch_snapshot(committed, scale);
        }
        let present_us = present_start.elapsed().as_micros() as u64;
        // The frame is posted before waiting: the FPGA consumes it on the next
        // vblank while the CPU prepares no writes to the committed slot.
        let vsync = pacer.wait();
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

fn load_screensaver_images(cap: usize) -> Vec<SaverImage> {
    let arcade_screenshot_pack =
        std::path::Path::new(mister_magik_catalog::catalog_config::DEFAULT_SQLITE_PATH)
            .parent()
            .expect("default catalog has an application directory")
            .join("assets/arcade-screenshots-320x320.mmlz4b");
    let mut asset_keys =
        match preview_worker::preview_archive_sidecar_entry_stems(&arcade_screenshot_pack) {
            Ok(Some(sidecar)) => sidecar.entries,
            Ok(None) => match preview_worker::preview_archive_index(&arcade_screenshot_pack) {
                Ok(index) => index.entries,
                Err(error) => {
                    crate::ui_errln!("screensaver: arcade screenshot pack index failed: {error}");
                    Vec::new()
                }
            },
            Err(error) => {
                crate::ui_errln!("screensaver: arcade screenshot sidecar failed: {error}");
                Vec::new()
            }
        };
    let mut rng = random_seed();
    shuffle(&mut asset_keys, &mut rng);
    let mut images = Vec::new();
    for asset_key in asset_keys {
        if images.len() >= cap {
            break;
        }
        if let Ok(image) = preview_worker::load_preview_asset_pixels(
            &arcade_screenshot_pack.display().to_string(),
            &asset_key,
        ) {
            let image = preview_pixels_to_saver_image(image);
            images.push(image);
        }
    }
    images
}

fn preview_pixels_to_saver_image(image: preview_worker::PreviewPixels) -> SaverImage {
    match image {
        preview_worker::PreviewPixels::Rgb565 {
            width,
            height,
            stride_bytes,
            words,
        } => SaverImage {
            pixels: words.iter().copied().map(Rgb565Pixel).collect(),
            w: width as usize,
            h: height as usize,
            stride: stride_bytes as usize / 2,
        },
    }
}

fn render_screensaver_frame(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    mode: ScreensaverMode,
    frame: u64,
) {
    if !matches!(
        mode,
        ScreensaverMode::AttractWall
            | ScreensaverMode::StarfieldCabinets
            | ScreensaverMode::TilemapMuseum
            | ScreensaverMode::PreviewPlasmaCollage
            | ScreensaverMode::PhosphorGrid
            | ScreensaverMode::ScannerContactSheet
            | ScreensaverMode::RandomAccessLoader
            | ScreensaverMode::ColorClashGallery
    ) {
        clear(dst, color565(2, 4, 10));
    }
    match mode {
        ScreensaverMode::AttractWall => render_attract_wall(dst, state, w, h, images, frame),
        ScreensaverMode::MvsCarousel => render_carousel(dst, w, h, images, frame),
        ScreensaverMode::SuperScalerFlyby => render_flyby(dst, w, h, images, frame),
        ScreensaverMode::StarfieldCabinets => {
            render_starfield_cabinets(dst, state, w, h, images, frame);
        }
        ScreensaverMode::ScreenshotRain => render_rain(dst, w, h, images, frame),
        ScreensaverMode::TilemapMuseum => render_tilemap(dst, state, w, h, images, frame),
        ScreensaverMode::RasterGallery => render_raster_gallery(dst, w, h, images, frame),
        ScreensaverMode::KefrensScreenshotBars => render_kefrens(dst, w, h, images, frame),
        ScreensaverMode::PreviewPlasmaCollage => {
            render_plasma_collage(dst, w, h, images, frame);
        }
        ScreensaverMode::PhosphorGrid => render_phosphor_grid(dst, state, w, h, images, frame),
        ScreensaverMode::WarpTunnel => render_warp(dst, w, h, images, frame),
        ScreensaverMode::Mode7Floor => render_mode7(dst, w, h, images, frame),
        ScreensaverMode::ScannerContactSheet => render_scanner(dst, state, w, h, images, frame),
        ScreensaverMode::SpriteMultiplexParade => {
            render_parade(dst, &mut state.parade, w, h, images, frame)
        }
        ScreensaverMode::CabinetMarquee => render_marquee(dst, w, h, images, frame),
        ScreensaverMode::RandomAccessLoader => {
            render_random_loader(dst, state, w, h, images, frame)
        }
        ScreensaverMode::ColorClashGallery => render_color_clash(dst, state, w, h, images, frame),
        ScreensaverMode::RadialStarfield => render_starfield(dst, w, h, frame),
        ScreensaverMode::IdleMegademo => {
            let sub =
                ScreensaverMode::ALL[((frame / 240) as usize) % (ScreensaverMode::ALL.len() - 1)];
            render_screensaver_frame(dst, state, w, h, images, sub, frame);
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
    if tint == 255
        && out_w == img.w
        && out_h == img.h
        && x >= 0
        && y >= 0
        && (x as usize + out_w) <= screen_w
        && (y as usize + out_h) <= screen_h
    {
        let x = x as usize;
        let y = y as usize;
        for yy in 0..out_h {
            let dst_row = (y + yy) * screen_w + x;
            let src_row = yy * img.stride;
            dst[dst_row..dst_row + out_w].copy_from_slice(&img.pixels[src_row..src_row + out_w]);
        }
        return;
    }

    let dx0 = x.max(0) as usize;
    let dy0 = y.max(0) as usize;
    let dx1 = (x + out_w as isize).clamp(0, screen_w as isize) as usize;
    let dy1 = (y + out_h as isize).clamp(0, screen_h as isize) as usize;
    if dx1 <= dx0 || dy1 <= dy0 {
        return;
    }

    let step_x_fp = ((img.w << 16) / out_w.max(1)).max(1);
    let step_y_fp = ((img.h << 16) / out_h.max(1)).max(1);
    let base_x_fp = (dx0 as isize - x).max(0) as usize * step_x_fp;
    let mut sy_fp = (dy0 as isize - y).max(0) as usize * step_y_fp;
    let dark = color565(0, 0, 18);
    for dy in dy0..dy1 {
        let sy = (sy_fp >> 16).min(img.h - 1);
        let mut sx_fp = base_x_fp;
        let dst_row = dy * screen_w;
        let src_row = sy * img.stride;
        if tint == 255 {
            for dx in dx0..dx1 {
                let sx = (sx_fp >> 16).min(img.w - 1);
                dst[dst_row + dx] = img.pixels[src_row + sx];
                sx_fp = sx_fp.saturating_add(step_x_fp);
            }
        } else {
            for dx in dx0..dx1 {
                let sx = (sx_fp >> 16).min(img.w - 1);
                dst[dst_row + dx] = blend_565(dark, img.pixels[src_row + sx], tint);
                sx_fp = sx_fp.saturating_add(step_x_fp);
            }
        }
        sy_fp = sy_fp.saturating_add(step_y_fp);
    }
}

fn blit_slice_scaled(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    img: &SaverImage,
    src_x0: usize,
    src_w: usize,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
) {
    if out_w == 0 || out_h == 0 || src_w == 0 {
        return;
    }
    let src_x0 = src_x0.min(img.w - 1);
    let src_w = src_w.min(img.w.saturating_sub(src_x0)).max(1);
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
            let sx = src_x0 + (xx * src_w / out_w).min(src_w - 1);
            let mut px = sample_image(img, sx, sy);
            if tint < 255 {
                px = blend_565(color565(0, 0, 18), px, tint);
            }
            dst[dy as usize * screen_w + dx as usize] = px;
        }
    }
}

fn fill_rect(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: Rgb565Pixel,
) {
    let x1 = (x + w).min(screen_w);
    let y1 = (y + h).min(screen_h);
    for yy in y.min(screen_h)..y1 {
        dst[yy * screen_w + x.min(screen_w)..yy * screen_w + x1].fill(color);
    }
}

fn copy_rect(
    dst: &mut [Rgb565Pixel],
    src: &[Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    let x1 = (x + w).min(screen_w);
    let y1 = (y + h).min(screen_h);
    for yy in y.min(screen_h)..y1 {
        let row = yy * screen_w;
        dst[row + x.min(screen_w)..row + x1].copy_from_slice(&src[row + x.min(screen_w)..row + x1]);
    }
}

fn copy_rect_from_to(
    dst: &mut [Rgb565Pixel],
    src: &[Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    src_x: usize,
    src_y: usize,
    dst_x: isize,
    dst_y: isize,
    w: usize,
    h: usize,
) {
    let dx0 = dst_x.max(0) as usize;
    let dy0 = dst_y.max(0) as usize;
    let dx1 = (dst_x + w as isize).clamp(0, screen_w as isize) as usize;
    let dy1 = (dst_y + h as isize).clamp(0, screen_h as isize) as usize;
    if dx1 <= dx0 || dy1 <= dy0 {
        return;
    }
    let sx0 = src_x + (dx0 as isize - dst_x) as usize;
    let sy0 = src_y + (dy0 as isize - dst_y) as usize;
    for row in 0..(dy1 - dy0) {
        let src_row = (sy0 + row) * screen_w + sx0;
        let dst_row = (dy0 + row) * screen_w + dx0;
        dst[dst_row..dst_row + (dx1 - dx0)].copy_from_slice(&src[src_row..src_row + (dx1 - dx0)]);
    }
}

fn stroke_rect(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: Rgb565Pixel,
) {
    if w == 0 || h == 0 {
        return;
    }
    fill_rect(dst, screen_w, screen_h, x, y, w, 2, color);
    fill_rect(
        dst,
        screen_w,
        screen_h,
        x,
        y.saturating_add(h.saturating_sub(2)),
        w,
        2,
        color,
    );
    fill_rect(dst, screen_w, screen_h, x, y, 2, h, color);
    fill_rect(
        dst,
        screen_w,
        screen_h,
        x.saturating_add(w.saturating_sub(2)),
        y,
        2,
        h,
        color,
    );
}

fn image_at(images: &[SaverImage], idx: usize) -> Option<&SaverImage> {
    if images.is_empty() {
        None
    } else {
        images.get(idx % images.len())
    }
}

fn render_attract_wall(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let slot_w = 320usize;
    let slot_h = 224usize;
    let gutter_y = (h.saturating_sub(slot_h * 2)) / 3;
    let page = (frame / 360) as usize;
    let active = ((frame / 60) as usize) % 6;
    let reveal = ((frame % 60) as usize * slot_w) / 60;
    if !state.attract_wall_valid
        || state.attract_wall_page != page
        || state.attract_wall_base.len() != dst.len()
    {
        state.attract_wall_base.resize(dst.len(), Rgb565Pixel(0));
        state.attract_wall_next.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.attract_wall_base, color565(2, 4, 10));
        clear(&mut state.attract_wall_next, color565(2, 4, 10));
        for slot in 0..6 {
            let col = slot % 3;
            let row = slot / 3;
            let x = col * slot_w;
            let y = gutter_y + row * (slot_h + gutter_y);
            fill_rect(
                &mut state.attract_wall_base,
                w,
                h,
                x,
                y,
                slot_w,
                slot_h,
                color565(0, 0, 0),
            );
            if let Some(img) = image_at(images, page * 6 + slot) {
                blit_scaled(
                    &mut state.attract_wall_base,
                    w,
                    h,
                    img,
                    x as isize,
                    y as isize,
                    slot_w,
                    slot_h,
                    230,
                );
            }
            if let Some(img) = image_at(images, (page + 1) * 6 + slot) {
                blit_scaled(
                    &mut state.attract_wall_next,
                    w,
                    h,
                    img,
                    x as isize,
                    y as isize,
                    slot_w,
                    slot_h,
                    255,
                );
            }
            stroke_rect(
                &mut state.attract_wall_base,
                w,
                h,
                x,
                y,
                slot_w,
                slot_h,
                color565(70, 255, 210),
            );
        }
        state.attract_wall_page = page;
        state.attract_wall_valid = true;
    }

    dst.copy_from_slice(&state.attract_wall_base);
    for slot in 0..6 {
        if slot != active || reveal == 0 {
            continue;
        }
        let col = slot % 3;
        let row = slot / 3;
        let x = col * slot_w;
        let y = gutter_y + row * (slot_h + gutter_y);
        copy_rect(dst, &state.attract_wall_next, w, h, x, y, reveal, slot_h);
        stroke_rect(dst, w, h, x, y, slot_w, slot_h, color565(70, 255, 210));
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
            let scale_idx = ((frame as usize / 12 + i * 2) % 6).max(1);
            let out_w = [64, 96, 128, 160, 224, 320][scale_idx];
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

fn render_horizontal_starfield(dst: &mut [Rgb565Pixel], w: usize, h: usize, frame: u64) {
    clear(dst, color565(0, 0, 10));
    for i in 0..420usize {
        let layer = i & 3;
        let x = horizontal_star_x(i, w, frame);
        let y = (i.wrapping_mul(83).wrapping_add(i.wrapping_mul(i) * 7)) % h;
        let brightness = [70, 110, 170, 235][layer];
        let color = color565(brightness / 2, brightness, 255);
        dst[y * w + x] = color;
        if layer == 3 && x + 1 < w {
            dst[y * w + x + 1] = color;
        }
    }
}

fn horizontal_star_x(star: usize, width: usize, frame: u64) -> usize {
    const STAR_SPEED_DENOMINATOR: u64 = 8;
    let speed_numerator = PARADE_MIN_TILE_SPEED as u64 * ((star & 3) + 1) as u64;
    let start_x = (star
        .wrapping_mul(197)
        .wrapping_add(star.wrapping_mul(star) * 13))
        % width;
    let travel = frame.saturating_mul(speed_numerator) / STAR_SPEED_DENOMINATOR;
    (start_x + travel as usize) % width
}

fn render_starfield_cabinets(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    render_starfield(dst, w, h, frame);
    let cols = 5usize;
    let rows = 3usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let contact_frame = frame / 2;
    let contact_start = (contact_frame / 90) as usize;
    if !state.starfield_contact_valid
        || state.starfield_contact_start != contact_start
        || state.starfield_contact.len() != dst.len()
    {
        state.starfield_contact.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.starfield_contact, Rgb565Pixel(0));
        for row in 0..rows {
            for col in 0..cols {
                if let Some(img) = image_at(images, contact_start + row * cols + col) {
                    let x = col * cell_w + 8;
                    let y = row * cell_h + 8;
                    let out_w = cell_w.saturating_sub(16);
                    let out_h = cell_h.saturating_sub(16);
                    blit_scaled(
                        &mut state.starfield_contact,
                        w,
                        h,
                        img,
                        x as isize,
                        y as isize,
                        out_w,
                        out_h,
                        230,
                    );
                    stroke_rect(
                        &mut state.starfield_contact,
                        w,
                        h,
                        x,
                        y,
                        out_w,
                        out_h,
                        color565(40, 250, 220),
                    );
                }
            }
        }
        state.starfield_contact_start = contact_start;
        state.starfield_contact_valid = true;
    }

    for row in 0..rows {
        let ox = ((contact_frame as usize + row * 13) & 31) as isize - 16;
        for col in 0..cols {
            let x = col * cell_w + 8;
            let y = row * cell_h + 8;
            copy_rect_from_to(
                dst,
                &state.starfield_contact,
                w,
                h,
                x,
                y,
                x as isize + ox,
                y as isize,
                cell_w.saturating_sub(16),
                cell_h.saturating_sub(16),
            );
        }
    }
}

fn render_rain(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    for y in 0..h {
        let c = (y * 60 / h) as u8;
        fill_rect(dst, w, h, 0, y, w, 1, color565(0, c / 3, c));
    }
    for i in 0..28 {
        if let Some(img) = image_at(images, i + frame as usize / 75) {
            let x = ((i * 47) % (w + 120)) as isize - 60;
            let y = ((i * 83 + frame as usize * (2 + i % 4)) % (h + 96)) as isize - 72;
            let (tw, th) = if i & 1 == 0 { (80, 56) } else { (120, 84) };
            blit_scaled(dst, w, h, img, x, y, tw, th, 205);
        }
    }
}

fn render_tilemap(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cols = 12usize;
    let rows = 8usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let page = (frame / 180) as usize;
    if !state.tilemap_valid || state.tilemap_page != page || state.tilemap_normal.len() != dst.len()
    {
        state.tilemap_normal.resize(dst.len(), Rgb565Pixel(0));
        state.tilemap_bright.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.tilemap_normal, color565(2, 4, 10));
        clear(&mut state.tilemap_bright, color565(2, 4, 10));
        for ty in 0..rows {
            for tx in 0..cols {
                if let Some(img) = image_at(images, page + ty * cols + tx) {
                    let x = (tx * cell_w) as isize;
                    let y = (ty * cell_h) as isize;
                    let out_w = cell_w.saturating_sub(2);
                    let out_h = cell_h.saturating_sub(2);
                    blit_scaled(
                        &mut state.tilemap_normal,
                        w,
                        h,
                        img,
                        x,
                        y,
                        out_w,
                        out_h,
                        185,
                    );
                    blit_scaled(
                        &mut state.tilemap_bright,
                        w,
                        h,
                        img,
                        x,
                        y,
                        out_w,
                        out_h,
                        255,
                    );
                }
            }
        }
        state.tilemap_page = page;
        state.tilemap_valid = true;
    }

    dst.copy_from_slice(&state.tilemap_normal);
    for ty in 0..rows {
        for tx in 0..cols {
            let flash = hash2_u8(tx + page, ty) < (frame as u8).wrapping_mul(3);
            if flash {
                copy_rect(
                    dst,
                    &state.tilemap_bright,
                    w,
                    h,
                    tx * cell_w,
                    ty * cell_h,
                    cell_w.saturating_sub(2),
                    cell_h.saturating_sub(2),
                );
            }
        }
    }
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
    if let Some(curr) = image_at(images, frame as usize / 240) {
        blit_scaled(dst, w, h, curr, 220, 70, 520, 390, 230);
    }
    if let Some(next) = image_at(images, frame as usize / 240 + 1) {
        let reveal_y = ((frame as usize % 180) * h) / 180;
        for y in (0..reveal_y).step_by(8) {
            blit_slice_scaled(dst, w, h, next, 0, next.w, 220, y as isize, 520, 6, 255);
        }
    }
    stroke_rect(dst, w, h, 218, 68, 524, 394, color565(255, 80, 200));
}

fn render_kefrens(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    let bar_w = 24usize;
    let bars = w / bar_w + 3;
    let mut y = 0usize;
    while y < h {
        let row = y * w;
        for bar in 0..bars {
            if let Some(img) = image_at(images, bar + frame as usize / 120) {
                let wave = triangle_wave_u8(y / 3 + bar * 5, frame as u8) as isize / 5 - 25;
                let x0 = bar as isize * bar_w as isize - bar_w as isize + wave;
                let x1 = x0 + bar_w as isize;
                if x1 <= 0 || x0 >= w as isize {
                    continue;
                }
                let dst_x0 = x0.max(0) as usize;
                let dst_x1 = x1.min(w as isize) as usize;
                let src_y = y * img.h / h;
                let src_row = src_y * img.stride;
                let src_base = (bar * 23 + frame as usize / 3) % img.w;
                for x in dst_x0..dst_x1 {
                    let local = (x as isize - x0) as usize;
                    let src_x = (src_base + local).min(img.w - 1);
                    dst[row + x] = img.pixels[src_row + src_x];
                }
            }
        }
        if y + 1 < h {
            let next = (y + 1) * w;
            let (head, tail) = dst.split_at_mut(next);
            tail[..w].copy_from_slice(&head[row..row + w]);
        }
        y += 2;
    }
}

fn render_plasma_collage(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let tile = 16usize;
    let page = frame as usize / 180;
    for y in (0..h).step_by(tile) {
        for x in (0..w).step_by(tile) {
            let selector = plasma_gate(x / tile, y / tile, frame as u8) as usize;
            if let Some(img) = image_at(images, page + selector / 32) {
                let sx = (x * img.w / w + selector) % img.w;
                let sy = (y * img.h / h + selector / 2) % img.h;
                let px = sample_image(img, sx, sy);
                fill_rect(dst, w, h, x, y, tile, tile, px);
            }
        }
    }
}

fn render_phosphor_grid(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cols = 12usize;
    let rows = 8usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let page = frame as usize / 240;
    if !state.phosphor_grid_valid
        || state.phosphor_grid_page != page
        || state.phosphor_grid.len() != dst.len()
    {
        state.phosphor_grid.resize(dst.len(), Rgb565Pixel(0));
        fill_rect(
            &mut state.phosphor_grid,
            w,
            h,
            0,
            0,
            w,
            h,
            color565(0, 18, 14),
        );
        for ty in 0..rows {
            for tx in 0..cols {
                if let Some(img) = image_at(images, page + ty * cols + tx) {
                    blit_scaled(
                        &mut state.phosphor_grid,
                        w,
                        h,
                        img,
                        (tx * cell_w) as isize,
                        (ty * cell_h) as isize,
                        cell_w.saturating_sub(2),
                        cell_h.saturating_sub(2),
                        105,
                    );
                }
            }
        }
        state.phosphor_grid_page = page;
        state.phosphor_grid_valid = true;
    }

    dst.copy_from_slice(&state.phosphor_grid);
    for y in (0..h).step_by(24) {
        fill_rect(dst, w, h, 0, y, w, 1, color565(30, 255, 180));
    }
    for x in (0..w).step_by(32) {
        fill_rect(dst, w, h, x, 0, 1, h, color565(20, 180, 150));
    }
    if frame % 180 < 12 {
        fill_rect(dst, w, h, 0, h / 2 - 3, w, 6, color565(180, 255, 230));
        fill_rect(dst, w, h, w / 2 - 3, 0, 6, h, color565(120, 255, 220));
    }
}

fn render_warp(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_starfield(dst, w, h, frame);
    for i in 0..12 {
        if let Some(img) = image_at(images, i + frame as usize / 150) {
            let inset_x = i * 34 + (frame as usize % 34);
            let inset_y = i * 18 + (frame as usize % 18);
            if inset_x * 2 >= w || inset_y * 2 >= h {
                continue;
            }
            let rw = w - inset_x * 2;
            let rh = h - inset_y * 2;
            blit_slice_scaled(
                dst,
                w,
                h,
                img,
                0,
                img.w,
                inset_x as isize,
                inset_y as isize,
                rw,
                4,
                180,
            );
            blit_slice_scaled(
                dst,
                w,
                h,
                img,
                0,
                img.w,
                inset_x as isize,
                (inset_y + rh.saturating_sub(4)) as isize,
                rw,
                4,
                180,
            );
            stroke_rect(dst, w, h, inset_x, inset_y, rw, rh, color565(60, 220, 255));
        }
    }
}

fn render_mode7(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    clear(dst, color565(4, 8, 22));
    if let Some(img) = image_at(images, frame as usize / 150) {
        let mut y = h / 2;
        while y < h {
            let depth = (y - h / 2 + 1) * 2;
            let span = (w * 80 / depth).max(1);
            let step_fp = ((span << 16) / w.max(1)).max(1);
            let mut sx_fp = 0usize;
            let base_x = (frame as usize * 2) % img.w;
            let sy = ((depth + frame as usize) / 3) % img.h;
            let row = y * w;
            for x in 0..w {
                let sx = (base_x + (sx_fp >> 16)) % img.w;
                dst[row + x] = sample_image(img, sx, sy);
                sx_fp = sx_fp.saturating_add(step_fp);
            }
            if y + 1 < h {
                let next = (y + 1) * w;
                let (head, tail) = dst.split_at_mut(next);
                tail[..w].copy_from_slice(&head[row..row + w]);
            }
            y += 2;
        }
    }
}

fn render_scanner(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cols = 7usize;
    let rows = 4usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let contact_frame = frame / 5;
    let contact_start = (contact_frame / 90) as usize;
    if !state.scanner_contact_valid
        || state.scanner_contact_start != contact_start
        || state.scanner_contact.len() != dst.len()
    {
        state.scanner_contact.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.scanner_contact, color565(2, 4, 10));
        for row in 0..rows {
            for col in 0..cols {
                if let Some(img) = image_at(images, contact_start + row * cols + col) {
                    let x = col * cell_w + 8;
                    let y = row * cell_h + 8;
                    let out_w = cell_w.saturating_sub(16);
                    let out_h = cell_h.saturating_sub(16);
                    blit_scaled(
                        &mut state.scanner_contact,
                        w,
                        h,
                        img,
                        x as isize,
                        y as isize,
                        out_w,
                        out_h,
                        230,
                    );
                    stroke_rect(
                        &mut state.scanner_contact,
                        w,
                        h,
                        x,
                        y,
                        out_w,
                        out_h,
                        color565(40, 250, 220),
                    );
                }
            }
        }
        state.scanner_contact_start = contact_start;
        state.scanner_contact_valid = true;
    }

    dst.copy_from_slice(&state.scanner_contact);
    let scan_y = (frame as usize * 5) % h;
    for y in scan_y.saturating_sub(3)..(scan_y + 4).min(h) {
        for x in 0..w {
            dst[y * w + x] = brighten_565(dst[y * w + x]);
        }
    }
    let active = (scan_y * 7 / h).min(6);
    if let Some(img) = image_at(images, active + frame as usize / 120) {
        blit_scaled(dst, w, h, img, 360, 170, 240, 180, 255);
        stroke_rect(dst, w, h, 358, 168, 244, 184, color565(255, 255, 255));
    }
}

const PARADE_TILE_COUNT: usize = 14;
const PARADE_TILE_W: usize = 96;
const PARADE_TILE_H: usize = 72;
const PARADE_MIN_TILE_SPEED: usize = 2;
const PARADE_SPEED_COUNT: usize = 4;

#[derive(Clone, Copy, Debug)]
struct ParadeTile {
    x: isize,
    y: usize,
    speed: usize,
    image_idx: usize,
}

struct ParadeState {
    tiles: Vec<ParadeTile>,
    deck: Vec<usize>,
    cursor: usize,
    rng: u64,
    image_count: usize,
}

impl ParadeState {
    fn new(seed: u64) -> Self {
        Self {
            tiles: Vec::new(),
            deck: Vec::new(),
            cursor: 0,
            rng: seed,
            image_count: 0,
        }
    }

    fn ensure_initialized(&mut self, image_count: usize, w: usize, h: usize) {
        if self.image_count == image_count && !self.tiles.is_empty() {
            return;
        }
        self.tiles.clear();
        self.deck = (0..image_count).collect();
        shuffle(&mut self.deck, &mut self.rng);
        self.cursor = 0;
        self.image_count = image_count;
        let tile_count = PARADE_TILE_COUNT.min(image_count);
        let mut initial_speeds = (0..tile_count)
            .map(|i| PARADE_MIN_TILE_SPEED + i % PARADE_SPEED_COUNT)
            .collect::<Vec<_>>();
        shuffle(&mut initial_speeds, &mut self.rng);
        for i in 0..tile_count {
            let Some(image_idx) = self.next_image_for(i) else {
                break;
            };
            let x = self.random_below(w + PARADE_TILE_W) as isize - PARADE_TILE_W as isize;
            let y = 40 + self.random_below(h.saturating_sub(120).max(1));
            self.tiles.push(ParadeTile {
                x,
                y,
                speed: initial_speeds[i],
                image_idx,
            });
        }
    }

    fn next_image_for(&mut self, replacing_tile: usize) -> Option<usize> {
        if self.deck.is_empty() {
            return None;
        }
        for _ in 0..self.deck.len() {
            if self.cursor == self.deck.len() {
                shuffle(&mut self.deck, &mut self.rng);
                self.cursor = 0;
            }
            let candidate = self.deck[self.cursor];
            self.cursor += 1;
            let already_visible = self
                .tiles
                .iter()
                .enumerate()
                .any(|(idx, tile)| idx != replacing_tile && tile.image_idx == candidate);
            if !already_visible {
                return Some(candidate);
            }
        }
        None
    }

    fn advance(&mut self, screen_w: usize, screen_h: usize) {
        for tile_idx in 0..self.tiles.len() {
            self.tiles[tile_idx].x += self.tiles[tile_idx].speed as isize;
            if self.tiles[tile_idx].x >= screen_w as isize {
                if let Some(image_idx) = self.next_image_for(tile_idx) {
                    let y = 40 + self.random_below(screen_h.saturating_sub(120).max(1));
                    let speed = PARADE_MIN_TILE_SPEED + self.random_below(PARADE_SPEED_COUNT);
                    let tile = &mut self.tiles[tile_idx];
                    tile.x = -(PARADE_TILE_W as isize);
                    tile.y = y;
                    tile.speed = speed;
                    tile.image_idx = image_idx;
                }
            }
        }
    }

    fn random_below(&mut self, upper: usize) -> usize {
        advance_rng(&mut self.rng) as usize % upper.max(1)
    }
}

fn random_seed() -> u64 {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    time ^ (std::process::id() as u64).rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15
}

fn shuffle<T>(values: &mut [T], rng: &mut u64) {
    for i in (1..values.len()).rev() {
        let j = (advance_rng(rng) as usize) % (i + 1);
        values.swap(i, j);
    }
}

fn advance_rng(rng: &mut u64) -> u64 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 7;
    *rng ^= *rng << 17;
    *rng
}

fn parade_draw_order(state: &ParadeState) -> ([usize; PARADE_TILE_COUNT], usize) {
    let mut order = [usize::MAX; PARADE_TILE_COUNT];
    let len = state.tiles.len().min(PARADE_TILE_COUNT);
    for (idx, slot) in order.iter_mut().take(len).enumerate() {
        *slot = idx;
    }
    order[..len].sort_unstable_by_key(|idx| state.tiles[*idx].speed);
    (order, len)
}

fn render_parade(
    dst: &mut [Rgb565Pixel],
    state: &mut ParadeState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    render_horizontal_starfield(dst, w, h, frame);
    state.ensure_initialized(images.len(), w, h);
    state.advance(w, h);
    let (draw_order, draw_count) = parade_draw_order(state);
    for tile_idx in draw_order.into_iter().take(draw_count) {
        let tile = &state.tiles[tile_idx];
        blit_scaled(
            dst,
            w,
            h,
            &images[tile.image_idx],
            tile.x,
            tile.y as isize,
            PARADE_TILE_W,
            PARADE_TILE_H,
            230,
        );
    }
}

fn render_marquee(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    fill_rect(dst, w, h, 0, 0, w, h, color565(2, 4, 16));
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
    if let Some(img) = image_at(images, frame as usize / 180) {
        blit_scaled(dst, w, h, img, 280, 150, 400, 260, 245);
        stroke_rect(dst, w, h, 276, 146, 408, 268, color565(255, 60, 180));
    }
}

fn render_random_loader(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let tiles_x = 15;
    let tiles_y = 9;
    let page = frame as usize / 180;
    if !state.random_loader_valid
        || state.random_loader_page != page
        || state.random_loader.len() != dst.len()
    {
        state.random_loader.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.random_loader, color565(0, 18, 30));
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_idx = tx + ty * tiles_x;
                if let Some(img) = image_at(images, page + tile_idx) {
                    blit_scaled(
                        &mut state.random_loader,
                        w,
                        h,
                        img,
                        (tx * w / tiles_x) as isize,
                        (ty * h / tiles_y) as isize,
                        w / tiles_x,
                        h / tiles_y,
                        240,
                    );
                }
            }
        }
        state.random_loader_page = page;
        state.random_loader_valid = true;
    }

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let idx = tx + ty * tiles_x + frame as usize / 30;
            let color = if hash2_u8(tx + idx, ty) > (frame as u8) {
                color565(0, 18, 30)
            } else {
                color565(20, 180, 210)
            };
            fill_rect(
                dst,
                w,
                h,
                tx * w / tiles_x,
                ty * h / tiles_y,
                w / tiles_x,
                h / tiles_y,
                color,
            );
        }
    }
    let loaded_tiles = ((frame as usize * 3) % (tiles_x * tiles_y)).max(1);
    for tile_idx in 0..loaded_tiles {
        let tx = tile_idx % tiles_x;
        let ty = tile_idx / tiles_x;
        copy_rect(
            dst,
            &state.random_loader,
            w,
            h,
            tx * w / tiles_x,
            ty * h / tiles_y,
            w / tiles_x,
            h / tiles_y,
        );
    }
}

fn render_color_clash(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cell = 16usize;
    let page = frame as usize / 180;
    for y in (0..h).step_by(cell) {
        for x in (0..w).step_by(cell) {
            if let Some(img) = image_at(images, page + x / cell + y / cell) {
                let sx = (x * img.w / w) % img.w;
                let sy = (y * img.h / h) % img.h;
                let sample = sample_image(img, sx, sy);
                let bright = if ((x / cell + y / cell + frame as usize / 12) & 1) == 0 {
                    color565(255, 230, 80)
                } else {
                    color565(40, 250, 220)
                };
                let dark = color565(10, 12, 30);
                let color = if (sample.0 & 0x0421) != 0 {
                    bright
                } else {
                    dark
                };
                fill_rect(dst, w, h, x, y, cell, cell, color);
            }
        }
    }
    if frame % 240 > 180 {
        let cols = 3usize;
        let rows = 2usize;
        let cell_w = w / cols;
        let cell_h = h / rows;
        let contact_start = (frame / 90) as usize;
        if !state.color_clash_contact_valid
            || state.color_clash_contact_start != contact_start
            || state.color_clash_contact.len() != dst.len()
        {
            state.color_clash_contact.resize(dst.len(), Rgb565Pixel(0));
            clear(&mut state.color_clash_contact, Rgb565Pixel(0));
            for row in 0..rows {
                for col in 0..cols {
                    if let Some(img) = image_at(images, contact_start + row * cols + col) {
                        let x = col * cell_w + 8;
                        let y = row * cell_h + 8;
                        let out_w = cell_w.saturating_sub(16);
                        let out_h = cell_h.saturating_sub(16);
                        blit_scaled(
                            &mut state.color_clash_contact,
                            w,
                            h,
                            img,
                            x as isize,
                            y as isize,
                            out_w,
                            out_h,
                            230,
                        );
                        stroke_rect(
                            &mut state.color_clash_contact,
                            w,
                            h,
                            x,
                            y,
                            out_w,
                            out_h,
                            color565(40, 250, 220),
                        );
                    }
                }
            }
            state.color_clash_contact_start = contact_start;
            state.color_clash_contact_valid = true;
        }
        for row in 0..rows {
            for col in 0..cols {
                copy_rect(
                    dst,
                    &state.color_clash_contact,
                    w,
                    h,
                    col * cell_w + 8,
                    row * cell_h + 8,
                    cell_w.saturating_sub(16),
                    cell_h.saturating_sub(16),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn parade_keeps_visible_games_unique_and_exhausts_the_pool_before_recycling() {
        let image_count = 32;
        let mut state = ParadeState::new(0x1234_5678_9abc_def0);
        state.ensure_initialized(image_count, 960, 540);
        assert_eq!(state.tiles.len(), PARADE_TILE_COUNT);

        let mut history = state
            .tiles
            .iter()
            .map(|tile| tile.image_idx)
            .collect::<BTreeSet<_>>();
        assert_eq!(history.len(), PARADE_TILE_COUNT);

        for tile_idx in 0..(image_count - PARADE_TILE_COUNT) {
            let slot = tile_idx % PARADE_TILE_COUNT;
            let next = state.next_image_for(slot).expect("unused game remains");
            state.tiles[slot].image_idx = next;
            assert!(history.insert(next), "game recycled before pool exhaustion");
            let visible = state
                .tiles
                .iter()
                .map(|tile| tile.image_idx)
                .collect::<BTreeSet<_>>();
            assert_eq!(visible.len(), state.tiles.len());
        }

        assert_eq!(history.len(), image_count);
    }

    #[test]
    fn parade_tile_identity_changes_only_after_it_leaves_the_screen() {
        let mut state = ParadeState::new(7);
        state.ensure_initialized(32, 960, 540);
        let original = state.tiles[0].image_idx;
        state.tiles[0].x = 958;
        state.tiles[0].speed = 1;

        state.advance(960, 540);
        assert_eq!(state.tiles[0].image_idx, original);
        assert_eq!(state.tiles[0].x, 959);

        state.advance(960, 540);
        assert_eq!(state.tiles[0].x, -(PARADE_TILE_W as isize));
        assert_ne!(state.tiles[0].image_idx, original);
    }

    #[test]
    fn parade_starfield_moves_horizontally_in_depth_bands() {
        let width = 960;
        for star in 0..4 {
            let x0 = horizontal_star_x(star, width, 0);
            let x1 = horizontal_star_x(star, width, 8);
            assert_eq!(
                (x1 + width - x0) % width,
                PARADE_MIN_TILE_SPEED * (star + 1)
            );
        }
    }

    #[test]
    fn fastest_star_layer_is_half_the_slowest_card_speed() {
        let width = 960;
        let x0 = horizontal_star_x(3, width, 0);
        let x1 = horizontal_star_x(3, width, 8);
        let star_travel = (x1 + width - x0) % width;
        let slowest_card_travel = PARADE_MIN_TILE_SPEED * 8;
        assert_eq!(star_travel * 2, slowest_card_travel);
    }

    #[test]
    fn parade_initializes_all_four_card_speeds_in_randomized_slots() {
        let mut state = ParadeState::new(0xfeed_face_cafe_beef);
        state.ensure_initialized(64, 960, 540);
        let mut counts = [0usize; PARADE_SPEED_COUNT];
        for tile in &state.tiles {
            counts[tile.speed - PARADE_MIN_TILE_SPEED] += 1;
        }
        assert!(counts.into_iter().all(|count| count >= 3));
        let mut other = ParadeState::new(0x0123_4567_89ab_cdef);
        other.ensure_initialized(64, 960, 540);
        assert_ne!(
            state
                .tiles
                .iter()
                .map(|tile| (tile.x, tile.y, tile.speed))
                .collect::<Vec<_>>(),
            other
                .tiles
                .iter()
                .map(|tile| (tile.x, tile.y, tile.speed))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parade_draws_faster_cards_above_slower_cards() {
        let mut state = ParadeState::new(11);
        state.ensure_initialized(64, 960, 540);
        for (tile, speed) in state.tiles.iter_mut().zip([5, 2, 4, 3].into_iter().cycle()) {
            tile.speed = speed;
        }
        let (order, len) = parade_draw_order(&state);
        let speeds = order
            .into_iter()
            .take(len)
            .map(|idx| state.tiles[idx].speed)
            .collect::<Vec<_>>();
        assert!(speeds.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(speeds.first(), Some(&2));
        assert_eq!(speeds.last(), Some(&5));
    }

    #[test]
    fn parade_render_places_faster_card_pixels_above_slower_card_pixels() {
        let slow = color565(255, 20, 20);
        let fast = color565(20, 255, 20);
        let images = vec![
            SaverImage {
                pixels: vec![slow],
                w: 1,
                h: 1,
                stride: 1,
            },
            SaverImage {
                pixels: vec![fast],
                w: 1,
                h: 1,
                stride: 1,
            },
        ];
        let mut state = ParadeState::new(13);
        state.image_count = images.len();
        state.deck = vec![0, 1];
        state.tiles = vec![
            ParadeTile {
                x: 10,
                y: 10,
                speed: 2,
                image_idx: 0,
            },
            ParadeTile {
                x: 10,
                y: 10,
                speed: 5,
                image_idx: 1,
            },
        ];
        let mut dst = vec![Rgb565Pixel(0); 160 * 120];

        render_parade(&mut dst, &mut state, 160, 120, &images, 1);

        assert_eq!(dst[20 * 160 + 20], blend_565(color565(0, 0, 18), fast, 230));
    }

    #[test]
    fn radial_starfield_is_available_as_a_standalone_mode() {
        assert_eq!(
            ScreensaverMode::parse("radial-starfield"),
            Some(ScreensaverMode::RadialStarfield)
        );
    }
}
