use super::*;

pub(super) const CONSOLE_LIST_X: usize = 40;
#[cfg(mister_bench_scenes)]
pub(super) const CONSOLE_LIST_Y: usize = 116;
#[cfg(mister_bench_scenes)]
pub(super) const CONSOLE_LIST_W: usize = 880;
#[cfg(mister_bench_scenes)]
pub(super) const CONSOLE_LIST_H: usize = 356;
#[cfg(mister_bench_scenes)]
pub(super) const CONSOLE_ROW_H: usize = 44;
#[cfg(mister_bench_scenes)]
pub(super) const CONSOLE_FONT_PX: f32 = 16.0;
#[cfg(mister_bench_scenes)]
pub(super) const CONSOLE_TRACE_DEFAULT_PATH: &str = "/tmp/mister-magik-console-scroll-trace.tsv";

#[cfg(mister_bench_scenes)]
pub(super) struct ConsoleScrollTrace {
    file: File,
    start: Instant,
    frame: u64,
    fb_sample_step: usize,
    copy_budget_us: u64,
}

#[cfg(mister_bench_scenes)]
pub(super) struct ConsoleScrollTraceSample {
    virtual_y: usize,
    slint_us: u64,
    ram_scroll_us: u64,
    strip_us: u64,
    vsync_wait_us: u64,
    fb_copy_us: u64,
    label_copy_us: u64,
    frame_wall_us: u64,
    copy_done_after_vsync_us: u64,
    fb_hash_us: u64,
    fb_hash: u64,
    fb_nonzero: u32,
}

#[cfg(mister_bench_scenes)]
impl ConsoleScrollTrace {
    pub(super) fn open(display_h: usize, list_y: usize) -> Option<Self> {
        let path = std::env::var("MISTER_CONSOLE_SCROLL_TRACE_FILE").ok()?;
        let path = if path.is_empty() {
            CONSOLE_TRACE_DEFAULT_PATH.to_string()
        } else {
            path
        };
        let mut file = File::create(&path).ok()?;
        let _ = writeln!(
            file,
            "frame\telapsed_ms\tvirtual_y\tslint_us\tram_scroll_us\tstrip_us\tvsync_wait_us\tfb_copy_us\tlabel_copy_us\tframe_wall_us\tcopy_done_after_vsync_us\tcopy_budget_us\tfb_hash_us\tfb_hash\tfb_nonzero"
        );
        let fb_sample_step = std::env::var("MISTER_CONSOLE_SCROLL_TRACE_STEP")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(32);
        let copy_budget_us = if display_h == 0 {
            0
        } else {
            ((list_y as u64) * 16_667) / (display_h as u64)
        };
        println!(
            "console_scroll trace: path={path} fb_sample_step={fb_sample_step} copy_budget_us={copy_budget_us}"
        );
        Some(Self {
            file,
            start: Instant::now(),
            frame: 0,
            fb_sample_step,
            copy_budget_us,
        })
    }

    pub(super) fn record(&mut self, sample: ConsoleScrollTraceSample) {
        let elapsed_ms = self.start.elapsed().as_millis();
        let _ = writeln!(
            self.file,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:016x}\t{}",
            self.frame,
            elapsed_ms,
            sample.virtual_y,
            sample.slint_us,
            sample.ram_scroll_us,
            sample.strip_us,
            sample.vsync_wait_us,
            sample.fb_copy_us,
            sample.label_copy_us,
            sample.frame_wall_us,
            sample.copy_done_after_vsync_us,
            self.copy_budget_us,
            sample.fb_hash_us,
            sample.fb_hash,
            sample.fb_nonzero
        );
        self.frame += 1;
    }
}

#[cfg(mister_bench_scenes)]
pub(super) fn run_console_scroll_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    app: slint_ui::console_scroll::ConsoleScroll,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let scale = ui.fb_scale();
    let fb_x = CONSOLE_LIST_X * scale;
    let fb_y = CONSOLE_LIST_Y * scale;
    let scroll_px = 2usize;
    let mut surface = vec![Pixel(0); CONSOLE_LIST_W * CONSOLE_LIST_H];
    let mut surface_y = 0usize;
    let mut font = ConsoleFont::new(CONSOLE_FONT_PX);
    let mut trace = ConsoleScrollTrace::open(disp.height(), fb_y);
    let mut pacer = VsyncPacer::from_env();
    let cpu = cpu_profile::start();

    window.request_redraw();
    update_slint_animations(animation_clock);
    window.draw_if_needed(|renderer| {
        let _ = renderer.render(&mut cached, ui.render_w());
    });
    copy_cached_rows(disp, ui, &cached, 0, ui.render_h());
    draw_console_virtual_strip(
        &mut surface,
        CONSOLE_LIST_W,
        CONSOLE_LIST_W,
        CONSOLE_LIST_H,
        0,
        0,
        &mut font,
    );
    copy_console_surface(disp, fb_x, fb_y, scale, &surface, surface_y);

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!("console_scroll running {label} — fb scroll-copy + exposed-strip redraw");

    let start = Instant::now();
    let mut frames = 0u64;
    let mut virtual_y = 0usize;
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut ram_scroll_us = 0u128;
    let mut strip_us = 0u128;
    let mut fb_copy_us = 0u128;
    let mut label_rect: Option<DirtyRect> = None;

    while secs == 0 || start.elapsed().as_secs() < secs {
        let frame_start = Instant::now();
        if fps_window_start.elapsed().as_millis() >= 1000 {
            let nn = fps_frames.max(1) as u128;
            let top_row = (virtual_y / CONSOLE_ROW_H) % 1000;
            app.set_fps_label(format!("fps {fps_frames}").into());
            app.set_blit_label(format!("ram scroll {}us", ram_scroll_us / nn).into());
            app.set_strip_label(format!("new strip {}us", strip_us / nn).into());
            app.set_row_label(format!("top row {top_row:03}").into());
            window.request_redraw();
            println!(
                "  fps ~ {fps_frames}  | ram-scroll {}us  exposed-strip {}us  fb-copy {}us  top-row {top_row}  vsync hits={} timeouts={} fallback={} errors={} hz={:.2}",
                ram_scroll_us / nn,
                strip_us / nn,
                fb_copy_us / nn,
                pacer.hits(),
                pacer.timeouts(),
                pacer.fallback_frames(),
                pacer.errors(),
                1_000_000.0 / pacer.period_us() as f64
            );
            fps_frames = 0;
            ram_scroll_us = 0;
            strip_us = 0;
            fb_copy_us = 0;
            fps_window_start = Instant::now();
        }

        update_slint_animations(animation_clock);
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, ui.render_w());
            label_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let t_slint_done = Instant::now();

        let t0 = Instant::now();
        surface_y = (surface_y + scroll_px) % CONSOLE_LIST_H;
        let t1 = Instant::now();
        virtual_y = virtual_y.wrapping_add(scroll_px);
        draw_console_virtual_strip_wrapped(
            &mut surface,
            CONSOLE_LIST_W,
            (surface_y + CONSOLE_LIST_H - scroll_px) % CONSOLE_LIST_H,
            scroll_px,
            virtual_y + CONSOLE_LIST_H - scroll_px,
            &mut font,
        );
        let t2 = Instant::now();

        let t_wait_start = Instant::now();
        let _pace = pacer.wait();
        let t3 = Instant::now();
        copy_console_surface(disp, fb_x, fb_y, scale, &surface, surface_y);
        let t4 = Instant::now();
        if let Some(rect) = label_rect.take() {
            copy_cached_rect(disp, ui, &cached, rect);
        }
        let t5 = Instant::now();
        if let Some(trace) = trace.as_mut() {
            let hash_start = Instant::now();
            let (fb_hash, fb_nonzero) = disp.rect_sampled_signature(
                fb_x,
                fb_y,
                CONSOLE_LIST_W * scale,
                CONSOLE_LIST_H * scale,
                trace.fb_sample_step,
            );
            let hash_end = Instant::now();
            trace.record(ConsoleScrollTraceSample {
                virtual_y,
                slint_us: (t_slint_done - frame_start).as_micros() as u64,
                ram_scroll_us: (t1 - t0).as_micros() as u64,
                strip_us: (t2 - t1).as_micros() as u64,
                vsync_wait_us: (t3 - t_wait_start).as_micros() as u64,
                fb_copy_us: (t4 - t3).as_micros() as u64,
                label_copy_us: (t5 - t4).as_micros() as u64,
                frame_wall_us: (t5 - frame_start).as_micros() as u64,
                copy_done_after_vsync_us: (t4 - t3).as_micros() as u64,
                fb_hash_us: (hash_end - hash_start).as_micros() as u64,
                fb_hash,
                fb_nonzero,
            });
        }

        frames += 1;
        fps_frames += 1;
        ram_scroll_us += (t1 - t0).as_micros();
        strip_us += (t2 - t1).as_micros();
        fb_copy_us += (t4 - t3).as_micros();
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

#[cfg(mister_bench_scenes)]
pub(super) fn copy_console_surface(
    disp: &mut Display,
    fb_x: usize,
    fb_y: usize,
    scale: usize,
    surface: &[Pixel],
    surface_y: usize,
) {
    if surface_y == 0 {
        disp.copy_rect_scaled_at(fb_x, fb_y, scale, surface, CONSOLE_LIST_W, CONSOLE_LIST_H);
        return;
    }

    let lower_h = CONSOLE_LIST_H - surface_y;
    disp.copy_rect_scaled_at(
        fb_x,
        fb_y,
        scale,
        &surface[surface_y * CONSOLE_LIST_W..],
        CONSOLE_LIST_W,
        lower_h,
    );
    disp.copy_rect_scaled_at(
        fb_x,
        fb_y + lower_h * scale,
        scale,
        surface,
        CONSOLE_LIST_W,
        surface_y,
    );
}

#[cfg(mister_bench_scenes)]
pub(super) fn draw_console_virtual_strip_wrapped(
    dst: &mut [Pixel],
    stride: usize,
    dst_y: usize,
    height: usize,
    virtual_y_start: usize,
    font: &mut ConsoleFont,
) {
    let first_h = height.min(CONSOLE_LIST_H - dst_y);
    draw_console_virtual_strip(
        dst,
        stride,
        CONSOLE_LIST_W,
        first_h,
        dst_y,
        virtual_y_start,
        font,
    );

    if first_h < height {
        draw_console_virtual_strip(
            dst,
            stride,
            CONSOLE_LIST_W,
            height - first_h,
            0,
            virtual_y_start + first_h,
            font,
        );
    }
}

#[cfg(mister_bench_scenes)]
pub(super) fn draw_console_virtual_strip(
    dst: &mut [Pixel],
    stride: usize,
    width: usize,
    height: usize,
    dst_y: usize,
    virtual_y_start: usize,
    font: &mut ConsoleFont,
) {
    let row_h = CONSOLE_ROW_H;
    for dy in 0..height {
        let vy = virtual_y_start + dy;
        let row = vy / row_h;
        let row_y = vy % row_h;
        let y = dst_y + dy;
        if y * stride >= dst.len() {
            break;
        }
        for dx in 0..width {
            let pos = y * stride + dx;
            if pos >= dst.len() {
                break;
            }
            dst[pos] = console_pixel(row, dx, row_y);
        }
    }

    let first_row = virtual_y_start / row_h;
    let last_row = (virtual_y_start + height.saturating_sub(1)) / row_h;
    for row in first_row..=last_row {
        let virtual_row_y = row * row_h;
        let row_screen_y = dst_y as isize + virtual_row_y as isize - virtual_y_start as isize;
        font.draw_text_clipped(
            dst,
            stride,
            width,
            dst_y,
            height,
            12,
            row_screen_y + 27,
            &format!("ROW {row:03}  MISTER GAME"),
            if row % 11 == 5 {
                Pixel(0x00fff2a8)
            } else {
                Pixel(0x00dbe7ff)
            },
        );
        font.draw_text_clipped(
            dst,
            stride,
            width,
            dst_y,
            height,
            CONSOLE_LIST_W as isize - 120,
            row_screen_y + 27,
            "COPY",
            Pixel(0x007dd3fc),
        );
    }
}

#[cfg(mister_bench_scenes)]
pub(super) fn console_pixel(row: usize, x: usize, y: usize) -> Pixel {
    let selected = row % 11 == 5;
    let bg = if selected {
        Pixel(0x003a2750)
    } else if row % 2 == 0 {
        Pixel(0x00101928)
    } else {
        Pixel(0x000b1220)
    };
    if y < 1 || y >= CONSOLE_ROW_H - 1 {
        return if selected {
            Pixel(0x00f5d76e)
        } else {
            Pixel(0x001f2d44)
        };
    }
    if x < 1 || x >= CONSOLE_LIST_W - 1 {
        return Pixel(0x00263752);
    }
    bg
}
