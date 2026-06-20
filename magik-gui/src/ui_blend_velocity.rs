//! Direct framebuffer blend/copy velocity benchmark.

use crate::arcade_catalog::ARCADE_ROW_HEIGHT;
use crate::arcade_list_renderer::{
    blend_row_towards, blend_velocity_fade_h_from_env, draw_arcade_row_background,
    fade_blend_constants, for_each_arcade_list_copy_segment, prune_arcade_row_cache,
    ArcadeListRenderer, CachedArcadeRow, FadeBlendConstants, ARCADE_LIST_FADE_COLOR,
    ARCADE_LIST_FADE_COLOR_565, ARCADE_LIST_FONT_PX, ARCADE_LIST_H, ARCADE_LIST_W, ARCADE_LIST_X,
    ARCADE_LIST_Y, ARCADE_ROW_CACHE_MAX, ARCADE_TITLE_GRADIENT,
};
use crate::bitmap_text::ConsoleFont;
use crate::cpu_profile;
use crate::fb::{pixel_to_rgb565, Display, Pixel, VsyncPacer};
use slint::platform::software_renderer::Rgb565Pixel;
use std::collections::HashMap;
use std::time::Instant;

fn blend_backend_label() -> &'static str {
    "scalar"
}

fn blend_velocity_title(idx: usize) -> String {
    const TITLES: &[&str] = &[
        "METAL SLUG",
        "STREET FIGHTER II",
        "DODONPACHI",
        "GAROU MARK OF THE WOLVES",
        "OUT RUN",
        "R-TYPE",
        "ALIEN VS PREDATOR",
        "BUBBLE BOBBLE",
        "FINAL FIGHT",
        "ESP RA.DE.",
        "THE KING OF FIGHTERS",
        "RAIDEN II",
    ];
    format!("{} {:03}", TITLES[idx % TITLES.len()], idx)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlendVelocityVariant {
    Baseline,
    CopyOnly,
    NoFade,
    RealText,
    GradientText,
}

impl BlendVelocityVariant {
    fn from_env() -> Self {
        match std::env::var("MISTER_BLEND_BENCH_VARIANT")
            .unwrap_or_else(|_| "baseline".into())
            .to_ascii_lowercase()
            .replace('_', "-")
            .as_str()
        {
            "copy-only" | "copy" => Self::CopyOnly,
            "no-fade" | "nofade" | "body-only" => Self::NoFade,
            "real-text" | "real_text" | "text" => Self::RealText,
            "gradient-text" | "gradient_text" | "gradient" => Self::GradientText,
            _ => Self::Baseline,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::CopyOnly => "copy-only",
            Self::NoFade => "no-fade",
            Self::RealText => "real-text",
            Self::GradientText => "gradient-text",
        }
    }

    fn uses_real_text_rows(self) -> bool {
        matches!(self, Self::RealText | Self::GradientText)
    }

    fn uses_viewport_fade(self) -> bool {
        !matches!(self, Self::NoFade | Self::GradientText)
    }
}

#[derive(Default)]
struct BlendVelocityTotals {
    frames: u64,
    surface_us: u128,
    fade_blend_us: u128,
    fade_copy_us: u128,
    body_copy_us: u128,
    selection_copy_us: u128,
    vsync_us: u128,
    wall_us: u128,
    rows: u128,
    px: u128,
}

impl BlendVelocityTotals {
    fn record(&mut self, sample: BlendVelocitySample) {
        self.frames += 1;
        self.surface_us += sample.surface_us as u128;
        self.fade_blend_us += sample.fade_blend_us as u128;
        self.fade_copy_us += sample.fade_copy_us as u128;
        self.body_copy_us += sample.body_copy_us as u128;
        self.selection_copy_us += sample.selection_copy_us as u128;
        self.vsync_us += sample.vsync_us as u128;
        self.wall_us += sample.wall_us as u128;
        self.rows += sample.rows as u128;
        self.px += sample.px as u128;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    fn avg(value: u128, frames: u64) -> u128 {
        if frames == 0 {
            0
        } else {
            value / frames as u128
        }
    }
}

#[derive(Clone, Copy)]
struct BlendVelocitySample {
    surface_us: u64,
    fade_blend_us: u64,
    fade_copy_us: u64,
    body_copy_us: u64,
    selection_copy_us: u64,
    vsync_us: u64,
    wall_us: u64,
    rows: u32,
    px: u32,
}

struct BlendVelocityBench {
    variant: BlendVelocityVariant,
    title_font: ConsoleFont,
    row_cache: HashMap<usize, CachedArcadeRow>,
    row_cache_epoch: u64,
    surface: Vec<Rgb565Pixel>,
    fade_scratch: Vec<Rgb565Pixel>,
    fade_constants: Vec<FadeBlendConstants>,
    fade_h: usize,
    selection_horizontal: Vec<Rgb565Pixel>,
    selection_vertical: Vec<Rgb565Pixel>,
    surface_y: usize,
    visual_px: i32,
    px_per_frame: i32,
}

impl BlendVelocityBench {
    fn new(variant: BlendVelocityVariant) -> Self {
        let mut this = Self {
            variant,
            title_font: ConsoleFont::new(ARCADE_LIST_FONT_PX),
            row_cache: HashMap::new(),
            row_cache_epoch: 0,
            surface: vec![ARCADE_LIST_FADE_COLOR_565; ARCADE_LIST_W * ARCADE_LIST_H],
            fade_scratch: Vec::new(),
            fade_constants: Vec::new(),
            fade_h: blend_velocity_fade_h_from_env(),
            selection_horizontal: Vec::new(),
            selection_vertical: Vec::new(),
            surface_y: 0,
            visual_px: 0,
            px_per_frame: std::env::var("MISTER_BLEND_BENCH_PX_PER_FRAME")
                .ok()
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(6),
        };
        this.fade_scratch = vec![Rgb565Pixel(0); ARCADE_LIST_W * this.fade_h];
        this.fade_constants = fade_blend_constants(this.fade_h, ARCADE_LIST_FADE_COLOR_565);
        this.draw_full_surface();
        this
    }

    fn run_frame(&mut self, disp: &mut Display, pacer: &mut VsyncPacer) -> BlendVelocitySample {
        let frame_start = Instant::now();
        let surface_start = Instant::now();
        self.advance_surface();
        let surface_us = surface_start.elapsed().as_micros() as u64;

        let pace = pacer.wait();
        let vsync_us = pace.wait_us;

        let mut rows = 0u32;
        let mut px = 0u32;

        let fade_blend_us = if self.variant.uses_viewport_fade()
            && self.variant != BlendVelocityVariant::CopyOnly
        {
            let fade_blend_start = Instant::now();
            let fade_blend_us = self.prepare_fade_bands();
            let measured_fade_blend_us = fade_blend_start.elapsed().as_micros() as u64;
            fade_blend_us.max(measured_fade_blend_us)
        } else {
            0
        };

        let fade_copy_start = Instant::now();
        let mut fade_copy_us = 0u64;
        if self.variant.uses_viewport_fade() {
            let fade_h = self.fade_h;
            let (top_rows, top_px) = self.copy_top_fade_to_display(disp, fade_h);
            let (bottom_rows, bottom_px) = self.copy_bottom_fade_to_display(disp, fade_h);
            rows += top_rows + bottom_rows;
            px += top_px + bottom_px;
            fade_copy_us = fade_copy_start.elapsed().as_micros() as u64;
        }

        let body_copy_start = Instant::now();
        let fade_h = self.fade_h;
        let body_y = if self.variant.uses_viewport_fade() {
            fade_h
        } else {
            0
        };
        let body_h = if self.variant.uses_viewport_fade() {
            ARCADE_LIST_H - fade_h * 2
        } else {
            ARCADE_LIST_H
        };
        let (body_rows, body_px) = self.copy_viewport_band_to_display(disp, body_y, body_h, true);
        let body_copy_us = body_copy_start.elapsed().as_micros() as u64;
        rows += body_rows;
        px += body_px;

        let selection_copy_us = 0;

        let wall_us = frame_start.elapsed().as_micros() as u64;
        BlendVelocitySample {
            surface_us,
            fade_blend_us,
            fade_copy_us,
            body_copy_us,
            selection_copy_us,
            vsync_us,
            wall_us,
            rows,
            px,
        }
    }

    fn advance_surface(&mut self) {
        let d = self.px_per_frame as usize;
        self.visual_px += self.px_per_frame;
        self.surface_y = (self.surface_y + d) % ARCADE_LIST_H;
        self.draw_band(ARCADE_LIST_H - d.min(ARCADE_LIST_H), d.min(ARCADE_LIST_H));
    }

    fn draw_full_surface(&mut self) {
        self.surface_y = 0;
        self.draw_band(0, ARCADE_LIST_H);
    }

    fn draw_band(&mut self, band_y: usize, band_h: usize) {
        if band_h == 0 {
            return;
        }
        let band_h = band_h.min(ARCADE_LIST_H - band_y);
        for row in 0..band_h {
            let viewport_y = band_y + row;
            let world_y = self.visual_px + viewport_y as i32;
            let row_idx = world_y.div_euclid(ARCADE_ROW_HEIGHT);
            let src_y = (world_y.rem_euclid(ARCADE_ROW_HEIGHT)) as usize;
            if self.variant.uses_real_text_rows() {
                self.copy_real_text_row_to_surface(row_idx.max(0) as usize, src_y, viewport_y);
                continue;
            }
            let bg = if row_idx % 2 == 0 {
                pixel_to_rgb565(Pixel(0x001a1424))
            } else {
                pixel_to_rgb565(Pixel(0x00150f20))
            };
            let border = pixel_to_rgb565(Pixel(0x00251c34));
            let dst_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
            let line = &mut self.surface[dst_y * ARCADE_LIST_W..(dst_y + 1) * ARCADE_LIST_W];
            line.fill(bg);
            if src_y == 0 || src_y == ARCADE_ROW_HEIGHT as usize - 1 {
                line.fill(border);
            }
            for x in 12..ARCADE_LIST_W.min(320) {
                if (x + row_idx as usize * 17 + src_y) % 37 == 0 {
                    line[x] = pixel_to_rgb565(Pixel(0x00e8e0f0));
                }
            }
        }
    }

    fn copy_real_text_row_to_surface(&mut self, idx: usize, src_y: usize, viewport_y: usize) {
        if !self.row_cache.contains_key(&idx) {
            if self.row_cache.len() >= ARCADE_ROW_CACHE_MAX {
                prune_arcade_row_cache(&mut self.row_cache);
            }
            let title = blend_velocity_title(idx);
            let mut row = vec![Pixel(0); ARCADE_LIST_W * ARCADE_ROW_HEIGHT as usize];
            draw_arcade_row_background(&mut row, idx);
            if self.variant == BlendVelocityVariant::GradientText {
                self.title_font.draw_text_clipped_gradient(
                    &mut row,
                    ARCADE_LIST_W,
                    ARCADE_LIST_W,
                    0,
                    ARCADE_ROW_HEIGHT as usize,
                    12,
                    30,
                    &title,
                    ARCADE_TITLE_GRADIENT,
                );
            } else {
                self.title_font.draw_text_clipped(
                    &mut row,
                    ARCADE_LIST_W,
                    ARCADE_LIST_W,
                    0,
                    ARCADE_ROW_HEIGHT as usize,
                    12,
                    30,
                    &title,
                    Pixel(0x00e8e0f0),
                );
            }
            let last_used = self.next_row_cache_epoch();
            self.row_cache.insert(
                idx,
                CachedArcadeRow {
                    title,
                    pixels: row.into_iter().map(pixel_to_rgb565).collect(),
                    last_used,
                },
            );
        } else {
            let last_used = self.next_row_cache_epoch();
            if let Some(cached) = self.row_cache.get_mut(&idx) {
                cached.last_used = last_used;
            }
        }
        let dst_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
        let dst = dst_y * ARCADE_LIST_W;
        let src = src_y * ARCADE_LIST_W;
        let row = &self.row_cache.get(&idx).expect("row cache insert").pixels;
        self.surface[dst..dst + ARCADE_LIST_W].copy_from_slice(&row[src..src + ARCADE_LIST_W]);
    }

    fn next_row_cache_epoch(&mut self) -> u64 {
        self.row_cache_epoch = self.row_cache_epoch.wrapping_add(1);
        self.row_cache_epoch
    }

    fn prepare_fade_bands(&mut self) -> u64 {
        let start = Instant::now();
        let fade_h = self.fade_h;
        self.fade_scratch
            .resize(ARCADE_LIST_W * fade_h * 2, Rgb565Pixel(0));
        let surface = &self.surface;
        let surface_y = self.surface_y;
        let fade_scratch = &mut self.fade_scratch;
        let fade_constants = &self.fade_constants;
        for row in 0..fade_h {
            let src_y = (surface_y + row) % ARCADE_LIST_H;
            let src = src_y * ARCADE_LIST_W;
            blend_row_towards(
                &surface[src..src + ARCADE_LIST_W],
                &mut fade_scratch[row * ARCADE_LIST_W..(row + 1) * ARCADE_LIST_W],
                fade_constants[row],
            );
        }
        for row in 0..fade_h {
            let viewport_y = ARCADE_LIST_H - fade_h + row;
            let dst_row = fade_h + row;
            let src_y = (surface_y + viewport_y) % ARCADE_LIST_H;
            let src = src_y * ARCADE_LIST_W;
            blend_row_towards(
                &surface[src..src + ARCADE_LIST_W],
                &mut fade_scratch[dst_row * ARCADE_LIST_W..(dst_row + 1) * ARCADE_LIST_W],
                fade_constants[fade_h - 1 - row],
            );
        }
        start.elapsed().as_micros() as u64
    }

    fn copy_top_fade_to_display(&mut self, disp: &mut Display, fade_h: usize) -> (u32, u32) {
        if self.variant == BlendVelocityVariant::CopyOnly {
            self.copy_viewport_band_to_display(disp, 0, fade_h, true)
        } else {
            copy_tight_band_to_display(
                disp,
                &self.fade_scratch[..ARCADE_LIST_W * fade_h],
                0,
                fade_h,
                true,
            )
        }
    }

    fn copy_bottom_fade_to_display(&mut self, disp: &mut Display, fade_h: usize) -> (u32, u32) {
        if self.variant == BlendVelocityVariant::CopyOnly {
            self.copy_viewport_band_to_display(disp, ARCADE_LIST_H - fade_h, fade_h, true)
        } else {
            let offset = ARCADE_LIST_W * fade_h;
            copy_tight_band_to_display(
                disp,
                &self.fade_scratch[offset..offset + ARCADE_LIST_W * fade_h],
                ARCADE_LIST_H - fade_h,
                fade_h,
                true,
            )
        }
    }

    fn copy_viewport_band_to_display(
        &mut self,
        disp: &mut Display,
        viewport_y: usize,
        h: usize,
        preserve_selection_frame: bool,
    ) -> (u32, u32) {
        if h == 0 || viewport_y >= ARCADE_LIST_H {
            return (0, 0);
        }
        let h = h.min(ARCADE_LIST_H - viewport_y);
        let mut rows = 0u32;
        let mut px = 0u32;
        for_each_arcade_list_copy_segment(viewport_y, h, preserve_selection_frame, |x, y, w, h| {
            self.copy_surface_rect_to_display(disp, x, y, w, h);
            rows += h as u32;
            px += (w * h) as u32;
        });
        (rows, px)
    }

    fn copy_surface_rect_to_display(
        &mut self,
        disp: &mut Display,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        let mut copied = 0usize;
        while copied < h {
            let src_y = (self.surface_y + viewport_y + copied) % ARCADE_LIST_H;
            let copy_h = (h - copied).min(ARCADE_LIST_H - src_y);
            self.copy_surface_chunk_to_display(disp, x, viewport_y + copied, w, copy_h);
            copied += copy_h;
        }
    }

    fn copy_surface_chunk_to_display(
        &mut self,
        disp: &mut Display,
        x: usize,
        viewport_y: usize,
        w: usize,
        h: usize,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let src_y = (self.surface_y + viewport_y) % ARCADE_LIST_H;
        if x == 0 && w == ARCADE_LIST_W {
            let src = src_y * ARCADE_LIST_W;
            disp.copy_rect_from_565(
                ARCADE_LIST_X,
                ARCADE_LIST_Y + viewport_y,
                ARCADE_LIST_W,
                h,
                &self.surface[src..src + h * ARCADE_LIST_W],
            );
            return;
        }
        disp.copy_rect_from_565_strided(
            ARCADE_LIST_X + x,
            ARCADE_LIST_Y + viewport_y,
            w,
            h,
            &self.surface,
            ARCADE_LIST_W,
            x,
            src_y,
        );
    }

    fn copy_selection_frame_to_display(&mut self, disp: &mut Display) {
        let rect = ArcadeListRenderer::selection_rect();
        let color = pixel_to_rgb565(Pixel(0x0006d6a0));
        let thickness = 3usize;
        let h = rect.y1.saturating_sub(rect.y0).min(ARCADE_LIST_H);
        self.selection_horizontal
            .resize(ARCADE_LIST_W * thickness, color);
        self.selection_horizontal.fill(color);
        disp.copy_rect_from_565(
            rect.x0,
            rect.y0,
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        disp.copy_rect_from_565(
            rect.x0,
            rect.y1.saturating_sub(thickness),
            ARCADE_LIST_W,
            thickness,
            &self.selection_horizontal,
        );
        self.selection_vertical.resize(thickness * h, color);
        self.selection_vertical.fill(color);
        disp.copy_rect_from_565(rect.x0, rect.y0, thickness, h, &self.selection_vertical);
        disp.copy_rect_from_565(
            rect.x1.saturating_sub(thickness),
            rect.y0,
            thickness,
            h,
            &self.selection_vertical,
        );
    }
}

fn copy_tight_band_to_display(
    disp: &mut Display,
    src: &[Rgb565Pixel],
    viewport_y: usize,
    h: usize,
    preserve_selection_frame: bool,
) -> (u32, u32) {
    if h == 0 || viewport_y >= ARCADE_LIST_H {
        return (0, 0);
    }
    let h = h.min(ARCADE_LIST_H - viewport_y);
    let mut rows = 0u32;
    let mut px = 0u32;
    for_each_arcade_list_copy_segment(viewport_y, h, preserve_selection_frame, |x, y, w, h| {
        let src_y = y - viewport_y;
        if x == 0 && w == ARCADE_LIST_W {
            let src_offset = src_y * ARCADE_LIST_W;
            disp.copy_rect_from_565(
                ARCADE_LIST_X,
                ARCADE_LIST_Y + y,
                ARCADE_LIST_W,
                h,
                &src[src_offset..src_offset + h * ARCADE_LIST_W],
            );
        } else {
            disp.copy_rect_from_565_strided(
                ARCADE_LIST_X + x,
                ARCADE_LIST_Y + y,
                w,
                h,
                src,
                ARCADE_LIST_W,
                x,
                src_y,
            );
        }
        rows += h as u32;
        px += (w * h) as u32;
    });
    (rows, px)
}

pub(crate) fn run_blend_velocity_loop(secs: u64, disp: &mut Display) {
    let variant = BlendVelocityVariant::from_env();
    let mut bench = BlendVelocityBench::new(variant);
    bench.copy_selection_frame_to_display(disp);
    let mut pacer = VsyncPacer::from_env();
    let cpu = cpu_profile::start();
    let start = Instant::now();
    let mut frames = 0u64;
    let mut totals = BlendVelocityTotals::default();
    let mut window_totals = BlendVelocityTotals::default();
    let mut fps_window_start = Instant::now();
    let trace_path = std::env::var("MISTER_BLEND_BENCH_TRACE").ok();
    let mut trace = trace_path.as_ref().and_then(|path| {
        let mut file = std::fs::File::create(path)
            .map_err(|e| eprintln!("blend_velocity trace: create {path} failed: {e}"))
            .ok()?;
        std::io::Write::write_all(
            &mut file,
            b"frame\telapsed_us\tvariant\tvisual_px\tpx_per_frame\tfade_h\tsurface_us\tfade_blend_us\tfade_copy_us\tbody_copy_us\tselection_copy_us\tvsync_us\twall_us\trows\tpx\n",
        )
        .map_err(|e| eprintln!("blend_velocity trace: header write failed: {e}"))
        .ok()?;
        println!("blend_velocity_trace={path}");
        Some(file)
    });

    println!(
        "blend_velocity running variant={} px_per_frame={} fade_h={} fade_target=#{:06x} blend_backend={} trace={} secs={}",
        variant.label(),
        bench.px_per_frame,
        bench.fade_h,
        ARCADE_LIST_FADE_COLOR.0 & 0x00ff_ffff,
        blend_backend_label(),
        trace_path.as_deref().unwrap_or("off"),
        secs
    );

    while secs == 0 || start.elapsed().as_secs() < secs {
        let sample = bench.run_frame(disp, &mut pacer);
        frames += 1;
        totals.record(sample);
        window_totals.record(sample);
        if let Some(file) = trace.as_mut() {
            let _ = std::io::Write::write_fmt(
                file,
                format_args!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    frames,
                    start.elapsed().as_micros(),
                    variant.label(),
                    bench.visual_px,
                    bench.px_per_frame,
                    bench.fade_h,
                    sample.surface_us,
                    sample.fade_blend_us,
                    sample.fade_copy_us,
                    sample.body_copy_us,
                    sample.selection_copy_us,
                    sample.vsync_us,
                    sample.wall_us,
                    sample.rows,
                    sample.px
                ),
            );
        }

        if fps_window_start.elapsed().as_millis() >= 1000 {
            let n = window_totals.frames.max(1);
            println!(
                "blend_velocity fps ~ {} variant={} surface {}us fade-blend {}us fade-copy {}us body-copy {}us selection-copy {}us vsync {}us wall {}us rows {} px {}",
                window_totals.frames,
                variant.label(),
                BlendVelocityTotals::avg(window_totals.surface_us, n),
                BlendVelocityTotals::avg(window_totals.fade_blend_us, n),
                BlendVelocityTotals::avg(window_totals.fade_copy_us, n),
                BlendVelocityTotals::avg(window_totals.body_copy_us, n),
                BlendVelocityTotals::avg(window_totals.selection_copy_us, n),
                BlendVelocityTotals::avg(window_totals.vsync_us, n),
                BlendVelocityTotals::avg(window_totals.wall_us, n),
                BlendVelocityTotals::avg(window_totals.rows, n),
                BlendVelocityTotals::avg(window_totals.px, n),
            );
            window_totals.reset();
            fps_window_start = Instant::now();
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let n = totals.frames.max(1);
    println!(
        "blend_velocity_result variant={} frames={} elapsed={elapsed:.1}s fps={:.1} surface_us={} fade_blend_us={} fade_copy_us={} body_copy_us={} selection_copy_us={} vsync_us={} wall_us={} rows={} px={}",
        variant.label(),
        frames,
        frames as f64 / elapsed,
        BlendVelocityTotals::avg(totals.surface_us, n),
        BlendVelocityTotals::avg(totals.fade_blend_us, n),
        BlendVelocityTotals::avg(totals.fade_copy_us, n),
        BlendVelocityTotals::avg(totals.body_copy_us, n),
        BlendVelocityTotals::avg(totals.selection_copy_us, n),
        BlendVelocityTotals::avg(totals.vsync_us, n),
        BlendVelocityTotals::avg(totals.wall_us, n),
        BlendVelocityTotals::avg(totals.rows, n),
        BlendVelocityTotals::avg(totals.px, n),
    );
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}
