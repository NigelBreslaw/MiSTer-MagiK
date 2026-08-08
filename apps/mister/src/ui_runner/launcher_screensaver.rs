// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg_attr(target_os = "macos", allow(dead_code))]

#[cfg(not(target_os = "macos"))]
use super::*;
use crate::preview_worker;
use mister_magik_catalog::device_layout::DeviceLayout;
use mister_magik_framebuffer_scenes::{Rgb565Pixel as SharedRgb565Pixel, SceneGeometry};
use mister_magik_screenshot_parade::{
    LiveScreenshotConfig, LiveScreenshotParade, ScreenshotBuffer, ScreenshotParade,
    ScreenshotParadeConfig, ScreenshotParadeStats,
};
#[cfg(target_os = "macos")]
use slint::platform::software_renderer::Rgb565Pixel;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

pub struct LauncherScreensaver {
    parade: Option<ScreenshotParade>,
    startup_started_at: Option<Instant>,
    frame: u64,
    motion_started_at: Instant,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScreensaverRenderTrace {
    pub(super) renderer: &'static str,
    pub(super) archive_poll_us: u128,
    pub(super) card_adopt_us: u128,
    pub(super) cards_adopted: usize,
    pub(super) parade_advance_us: u128,
    pub(super) background_us: u128,
    pub(super) draw_order_us: u128,
    pub(super) tile_blit_us: u128,
    pub(super) cards_drawn: usize,
    pub(super) cards_culled: usize,
    pub(super) raster_held_cards: usize,
    pub(super) raster_moved_cards: usize,
    pub(super) raster_hold_layer_mask: u8,
    pub(super) raster_visible_layer_mask: u8,
    pub(super) phase_bank_resident_bytes: usize,
}

pub(crate) fn shared_parade_trace(stats: ScreenshotParadeStats) -> ScreensaverRenderTrace {
    ScreensaverRenderTrace {
        card_adopt_us: stats.card_adopt_us,
        cards_adopted: stats.cards_adopted,
        parade_advance_us: stats.parade_advance_us,
        background_us: stats.background_us,
        draw_order_us: stats.draw_order_us,
        tile_blit_us: stats.tile_blit_us,
        cards_drawn: stats.cards_drawn,
        cards_culled: stats.cards_culled,
        raster_held_cards: stats.raster_held_cards,
        raster_moved_cards: stats.raster_moved_cards,
        raster_hold_layer_mask: stats.raster_hold_layer_mask,
        raster_visible_layer_mask: stats.raster_visible_layer_mask,
        phase_bank_resident_bytes: stats.phase_bank_resident_bytes,
        ..ScreensaverRenderTrace::default()
    }
}

fn log_shared_parade_stats(parade: &ScreenshotParade) {
    let stats = parade.stats();
    let scale_average_us = stats.scale_total_us / u128::from(stats.scale_count.max(1));
    let phase_average_us = stats.phase_total_us / u128::from(stats.phase_count.max(1));
    crate::ui_logln!(
        "screensaver_lanczos scales={} total_us={} average_us={} max_us={} phase_prepares={} phase_total_us={} phase_average_us={} phase_max_us={} queue_max={} queue_bound={} worker_connected=true phase_cache_bytes={}",
        stats.scale_count,
        stats.scale_total_us,
        scale_average_us,
        stats.scale_max_us,
        stats.phase_count,
        stats.phase_total_us,
        phase_average_us,
        stats.phase_max_us,
        stats.queue_max,
        parade.queue_bound(),
        stats.phase_bank_resident_bytes + stats.image_cache_resident_bytes
    );
    crate::ui_logln!(
        "screensaver_archive_runtime entries={} decodes={} failures={} unique_keys={} queue_depth={} queue_max={}",
        parade.asset_count(),
        stats.decode_successes,
        stats.decode_failures,
        stats.unique_decoded,
        stats.queue_depth,
        stats.queue_max
    );
}

fn slint_rgb565_as_shared_mut(destination: &mut [Rgb565Pixel]) -> &mut [SharedRgb565Pixel] {
    // SAFETY: both RGB565 pixel types are transparent `u16` wrappers with equal
    // size/alignment, and the mutable slice retains the input slice's lifetime.
    unsafe {
        std::slice::from_raw_parts_mut(
            destination.as_mut_ptr().cast::<SharedRgb565Pixel>(),
            destination.len(),
        )
    }
}

const _: () = {
    assert!(std::mem::size_of::<Rgb565Pixel>() == std::mem::size_of::<SharedRgb565Pixel>());
    assert!(std::mem::align_of::<Rgb565Pixel>() == std::mem::align_of::<SharedRgb565Pixel>());
};

fn shared_rgb565_into_slint(mut pixels: Vec<SharedRgb565Pixel>) -> Vec<Rgb565Pixel> {
    let length = pixels.len();
    let capacity = pixels.capacity();
    let pointer = pixels.as_mut_ptr().cast::<Rgb565Pixel>();
    std::mem::forget(pixels);
    // SAFETY: the compile-time assertions above establish identical layout.
    unsafe { Vec::from_raw_parts(pointer, length, capacity) }
}

fn slint_rgb565_into_shared(mut pixels: Vec<Rgb565Pixel>) -> Vec<SharedRgb565Pixel> {
    let length = pixels.len();
    let capacity = pixels.capacity();
    let pointer = pixels.as_mut_ptr().cast::<SharedRgb565Pixel>();
    std::mem::forget(pixels);
    // SAFETY: the compile-time assertions above establish identical layout.
    unsafe { Vec::from_raw_parts(pointer, length, capacity) }
}

pub(crate) struct LauncherScreenshotBuffer {
    pixels: Vec<SharedRgb565Pixel>,
}

impl LauncherScreenshotBuffer {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![SharedRgb565Pixel(0); width.saturating_mul(height)],
        }
    }

    pub(crate) fn into_pixels(self) -> Vec<Rgb565Pixel> {
        shared_rgb565_into_slint(self.pixels)
    }

    pub(crate) fn from_pixels(pixels: Vec<Rgb565Pixel>) -> Self {
        Self {
            pixels: slint_rgb565_into_shared(pixels),
        }
    }
}

impl ScreenshotBuffer for LauncherScreenshotBuffer {
    fn pixels_mut(&mut self) -> &mut [SharedRgb565Pixel] {
        &mut self.pixels
    }
}

pub(crate) type LauncherScreenshotRuntime = LiveScreenshotParade<LauncherScreenshotBuffer>;

impl LauncherScreensaver {
    pub fn render(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
    ) -> ScreensaverRenderTrace {
        let now = Instant::now();
        self.render_at(
            dst,
            w,
            h,
            now.saturating_duration_since(self.motion_started_at),
        )
    }

    pub fn render_at(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        elapsed: Duration,
    ) -> ScreensaverRenderTrace {
        self.render_at_target(dst, w, h, elapsed, None)
    }

    pub fn render_at_presentation_tick(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        presentation_tick: u64,
        fallback_elapsed: Duration,
    ) -> ScreensaverRenderTrace {
        self.render_at_target(dst, w, h, fallback_elapsed, Some(presentation_tick))
    }

    fn render_at_target(
        &mut self,
        dst: &mut [Rgb565Pixel],
        _w: usize,
        _h: usize,
        elapsed: Duration,
        presentation_tick: Option<u64>,
    ) -> ScreensaverRenderTrace {
        let mut trace = if let Some(parade) = self.parade.as_mut() {
            let render_result = match presentation_tick {
                Some(tick) => {
                    parade.render_at_presentation_tick(slint_rgb565_as_shared_mut(dst), tick)
                }
                None => parade.render_at(slint_rgb565_as_shared_mut(dst), elapsed),
            };
            match render_result {
                Ok(stats) => {
                    if parade.is_ready() {
                        if let Some(started) = self.startup_started_at.take() {
                            crate::ui_logln!(
                                "screensaver_startup_timing milestone=first_card_ready elapsed_us={} layer={}",
                                started.elapsed().as_micros(),
                                parade.first_ready_layer().unwrap_or_default()
                            );
                        }
                    }
                    shared_parade_trace(stats)
                }
                Err(error) => {
                    dst.fill(Rgb565Pixel(0));
                    crate::ui_errln!("screenshot parade render failed: {error}");
                    ScreensaverRenderTrace::default()
                }
            }
        } else {
            dst.fill(Rgb565Pixel(0));
            ScreensaverRenderTrace::default()
        };
        trace.renderer = "parade";
        if self.frame > 0 && self.frame % 600 == 0 {
            if let Some(parade) = self.parade.as_ref() {
                log_shared_parade_stats(parade);
            }
        }
        self.frame = self.frame.wrapping_add(1);
        trace
    }

    pub fn has_rendered_card(&self) -> bool {
        self.parade.as_ref().is_some_and(ScreenshotParade::is_ready)
    }

    pub fn is_loading_archive(&self) -> bool {
        false
    }

    pub fn active_card_count(&self) -> usize {
        self.parade
            .as_ref()
            .map_or(0, ScreenshotParade::active_card_count)
    }

    pub fn has_pending_card_work(&self) -> bool {
        self.parade
            .as_ref()
            .is_some_and(ScreenshotParade::has_pending_work)
    }

    pub fn preparation_slack(
        &self,
    ) -> Option<Arc<mister_magik_screenshot_parade::PreparationSlack>> {
        self.parade
            .as_ref()
            .and_then(ScreenshotParade::preparation_slack)
    }
}

impl LauncherScreensaver {
    pub fn from_archive_path(
        path: &std::path::Path,
        width: usize,
        height: usize,
        seed: u64,
    ) -> Result<Self, String> {
        let archive = preview_worker::ResidentPreviewArchive::open(path)?;
        let geometry =
            SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        let parade = ScreenshotParade::new(
            archive,
            ScreenshotParadeConfig {
                geometry,
                seed,
                worker_start: None,
                preparation_slack: None,
            },
        )?;
        let now = Instant::now();
        Ok(Self {
            parade: Some(parade),
            startup_started_at: None,
            frame: 0,
            motion_started_at: now,
        })
    }
}

pub struct LauncherScreensaverLoader {
    ready_rx: Receiver<LauncherScreenshotRuntime>,
    cancelled: Arc<AtomicBool>,
}

impl LauncherScreensaverLoader {
    pub fn start(w: usize, h: usize, startup_started_at: Option<Instant>) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        std::thread::Builder::new()
            .name("screensaver-load".into())
            .spawn(move || {
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverLoader,
                );
                let started = Instant::now();
                let path = screensaver_archive_path(
                    std::env::var_os("MISTER_MEDIA_ASSET_DIR").as_deref(),
                    DeviceLayout::current(),
                );
                let result: Result<Option<LauncherScreenshotRuntime>, String> = (|| {
                    let archive = preview_worker::ResidentPreviewArchive::open(&path)
                        .map_err(|error| format!("path={} error={error}", path.display()))?;
                    if worker_cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let open_us = started.elapsed().as_micros();
                    crate::ui_logln!(
                        "screensaver_loader path={} pack_bytes={} entries={}",
                        path.display(),
                        archive.compressed_bytes(),
                        archive.asset_keys().len()
                    );
                    let construction_started = Instant::now();
                    let seed = random_seed();
                    let buffers = std::array::from_fn(|_| LauncherScreenshotBuffer::new(w, h));
                    let mut runtime = LiveScreenshotParade::start(
                        archive,
                        LiveScreenshotConfig {
                            geometry: SceneGeometry::new(w, h, w)
                                .map_err(|error| error.to_string())?,
                            seed,
                            scale_worker_start: Some(Arc::new(|| {
                                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverScaler,
                                );
                            })),
                            render_worker_start: Some(Arc::new(|| {
                                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverRenderer,
                                );
                            })),
                        },
                        buffers,
                    )?;
                    runtime.wait_until_prefilled(Duration::from_secs(30))?;
                    runtime.finish_prefill()?;
                    let construct_us = construction_started.elapsed().as_micros();
                    crate::ui_logln!(
                        "screensaver_loader_timing archive_open_us={} runtime_prefill_us={} total_us={} cards=real",
                        open_us,
                        construct_us,
                        open_us.saturating_add(construct_us)
                    );
                    if let Some(started) = startup_started_at {
                        crate::ui_logln!(
                            "screensaver_startup_timing milestone=two_real_frames_ready elapsed_us={}",
                            started.elapsed().as_micros()
                        );
                    }
                    Ok(Some(runtime))
                })();
                match result {
                    Ok(Some(saver)) if !worker_cancelled.load(Ordering::Relaxed) => {
                        let _ = ready_tx.send(saver);
                    }
                    Ok(_) => {}
                    Err(error) => crate::ui_errln!("screensaver_loader error={error}"),
                }
            })
            .expect("spawn screensaver loader");
        Self {
            ready_rx,
            cancelled,
        }
    }

    pub(crate) fn try_ready(&self) -> Option<LauncherScreenshotRuntime> {
        self.ready_rx.try_recv().ok()
    }
}

impl Drop for LauncherScreensaverLoader {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

fn screensaver_archive_path(asset_dir: Option<&OsStr>, layout: DeviceLayout) -> PathBuf {
    asset_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| layout.app_path("assets"))
        .join("arcade-screenshots-320x320.mmlz4b")
}

fn random_seed() -> u64 {
    if let Some(seed) = std::env::var("MISTER_SCREENSAVER_SEED")
        .ok()
        .as_deref()
        .and_then(parse_screensaver_seed)
    {
        return seed;
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    time ^ (std::process::id() as u64).rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15
}

fn parse_screensaver_seed(value: &str) -> Option<u64> {
    let value = value.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|digits| u64::from_str_radix(digits, 16).ok())
        .unwrap_or_else(|| value.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screensaver_archive_path_uses_public_layout_by_default() {
        assert_eq!(
            screensaver_archive_path(None, DeviceLayout::Public),
            PathBuf::from("/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b")
        );
    }

    #[test]
    fn screensaver_archive_path_uses_development_layout_by_default() {
        assert_eq!(
            screensaver_archive_path(None, DeviceLayout::Dev),
            PathBuf::from("/media/fat/mister-magik-dev/assets/arcade-screenshots-320x320.mmlz4b")
        );
    }

    #[test]
    fn screensaver_archive_path_honors_explicit_asset_directory() {
        assert_eq!(
            screensaver_archive_path(
                Some(OsStr::new("/tmp/screensaver-assets")),
                DeviceLayout::Dev
            ),
            PathBuf::from("/tmp/screensaver-assets/arcade-screenshots-320x320.mmlz4b")
        );
    }

    #[test]
    fn public_archive_constructor_runs_the_production_parade() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-production-screensaver-{}.mmlz4b",
            std::process::id()
        ));
        write_single_image_archive(&path);
        let mut screensaver = LauncherScreensaver::from_archive_path(&path, 320, 180, 0x1234)
            .expect("open production screensaver");
        let mut frame = vec![Rgb565Pixel(0); 320 * 180];
        let trace = screensaver.render_at(&mut frame, 320, 180, Duration::from_secs(2));

        assert_eq!(trace.renderer, "parade");
        assert!(!screensaver.is_loading_archive());
        let _ = std::fs::remove_file(path);
    }

    fn write_single_image_archive(path: &std::path::Path) {
        let name = b"fixture.rgb565";
        let width = 2_u32;
        let height = 2_u32;
        let stride_bytes = 4_u32;
        let pixels = [0x00_u8, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xff, 0xff];
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MMPX2B1\0");
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride_bytes.to_le_bytes());
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&pixels);
        std::fs::write(path, bytes).expect("write production screensaver archive fixture");
    }

    #[test]
    fn benchmark_seed_accepts_decimal_and_hex_without_fallback_guessing() {
        assert_eq!(parse_screensaver_seed("42"), Some(42));
        assert_eq!(parse_screensaver_seed(" 0x2a "), Some(42));
        assert_eq!(parse_screensaver_seed("0X2A"), Some(42));
        assert_eq!(parse_screensaver_seed(""), None);
        assert_eq!(parse_screensaver_seed("seed"), None);
    }
}
