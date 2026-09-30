// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! The production rasterizers with fixed artwork, screens and timelines.
use crate::{Effect, Pixel, Preset, Rect, full};
use mister_magik_framebuffer_scenes::{
    arcade_card::{
        CABINET_HEIGHT, CABINET_WIDTH, CabinetTexture, render_arcade_card_filtered_band_into,
        render_arcade_card_filtered_into, render_arcade_card_transition_into,
    },
    launcher::{
        CardRenderQuality, LauncherFramePreparer, LauncherFrameRequest, PreparedLauncher,
        PreparedLauncherFrame,
    },
    launcher_navigation::{BrowseDirection, BrowseFrame, BrowsePhase},
    navigation::NavigationTransitionRect,
};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
const W: usize = 960;
const H: usize = 540;
const CARD: NavigationTransitionRect = NavigationTransitionRect {
    x: 520,
    y: 158,
    width: 180,
    height: 252,
};
pub(super) struct Lab {
    launcher: Option<PreparedLauncher>,
    arcade: Option<Vec<Pixel>>,
    cabinet: Vec<Pixel>,
    filtered: Option<CabinetTexture>,
    home: Vec<Pixel>,
    tiles: Option<ParallelTiles>,
    reveal: Option<ParallelReveal>,
    cache: Vec<Vec<Pixel>>,
    preparation: Vec<(&'static str, u64)>,
}
impl Lab {
    pub(super) fn new(
        name: &str,
        preset: Preset,
        worker_setup: Option<fn()>,
    ) -> Result<Self, String> {
        let quality = match preset {
            Preset::Default => CardRenderQuality::Current,
            Preset::Dithered => CardRenderQuality::Dithered,
            Preset::Rgb888 | Preset::Cached | Preset::CachedFast | Preset::Scanline => {
                CardRenderQuality::Rgb888
            }
            Preset::Reduced => return Err("reduced is not a rendering comparison".into()),
        };
        let mut preparation = Vec::new();
        let mut stage = Instant::now();
        let (launcher, home, cached_home) = if name == "launcher-cards" {
            let launcher = crate::fixture::prepare_quality(W, H, quality);
            let home = launcher.pixels().to_vec();
            (Some(launcher), home, false)
        } else {
            let (home, cached) = crate::fixture::home_quality(quality);
            (None, home, cached)
        };
        preparation.push((
            if cached_home {
                "launcher_snapshot"
            } else {
                "launcher_fixture_cold"
            },
            stage.elapsed().as_millis() as u64,
        ));
        stage = Instant::now();
        let bytes = include_bytes!("../../../apps/mister/assets/ui/arcade/cabinet-483x519.rgb565");
        let cabinet = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| Pixel(u16::from_le_bytes(*p)))
            .collect::<Vec<_>>();
        let (arcade, filtered) = if name == "arcade-transition" {
            let source = include_bytes!(
                "../../../apps/mister/tests/visual-baselines/launcher/hdmi-arcade.png"
            );
            let mut decoder = png::Decoder::new(Cursor::new(source));
            decoder.set_transformations(png::Transformations::EXPAND);
            let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
            let mut bytes = vec![
                0;
                reader
                    .output_buffer_size()
                    .ok_or("unbounded Arcade snapshot")?
            ];
            let info = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
            if (info.width, info.height) != (W as u32, H as u32)
                || info.bit_depth != png::BitDepth::Eight
            {
                return Err("Arcade snapshot geometry or bit depth changed".into());
            }
            let channels = match info.color_type {
                png::ColorType::Rgb => 3,
                png::ColorType::Rgba => 4,
                _ => return Err("Arcade snapshot must be RGB or RGBA".into()),
            };
            let mut frame: Vec<Pixel> = bytes[..info.buffer_size()]
                .chunks_exact(channels)
                .map(|p| {
                    Pixel(
                        (u16::from(p[0]) >> 3) << 11
                            | (u16::from(p[1]) >> 2) << 5
                            | u16::from(p[2]) >> 3,
                    )
                })
                .collect();
            preparation.push(("destination_decode", stage.elapsed().as_millis() as u64));
            stage = Instant::now();
            let filtered = match preset {
                Preset::Default => None,
                Preset::Dithered => Some(CabinetTexture::from_rgb565(&cabinet)?),
                Preset::Rgb888 | Preset::Cached | Preset::CachedFast | Preset::Scanline => {
                    Some(CabinetTexture::from_rgb888(include_bytes!(
                        "../../../apps/mister/assets/ui/arcade/cabinet-483x519.rgb888"
                    ))?)
                }
                Preset::Reduced => unreachable!(),
            };
            let filtered = filtered.map(|texture| {
                if matches!(preset, Preset::Scanline | Preset::CachedFast) {
                    texture.with_scanlines()
                } else {
                    texture
                }
            });
            if let Some(texture) = &filtered
                && !texture.prepare_destination(&mut frame)
            {
                return Err("resting cabinet preparation failed".into());
            }
            preparation.push(("cabinet_texture", stage.elapsed().as_millis() as u64));
            stage = Instant::now();
            (Some(frame), filtered)
        } else {
            (None, None)
        };
        assert_eq!(cabinet.len(), CABINET_WIDTH * CABINET_HEIGHT);
        let tiles = if name == "launcher-cards" {
            Some(ParallelTiles::new(
                launcher.as_ref().unwrap().frame_preparer(),
                worker_setup,
            )?)
        } else {
            None
        };
        let reveal = if matches!(preset, Preset::Scanline | Preset::CachedFast) {
            Some(ParallelReveal::new(
                &home,
                arcade.as_ref().unwrap(),
                filtered.as_ref().unwrap(),
                worker_setup,
            )?)
        } else {
            None
        };
        preparation.push(("worker_setup", stage.elapsed().as_millis() as u64));
        stage = Instant::now();
        let mut lab = Self {
            tiles,
            reveal,
            launcher,
            arcade,
            cabinet,
            filtered,
            home,
            cache: Vec::new(),
            preparation,
        };
        if matches!(preset, Preset::Cached | Preset::CachedFast) {
            // Store only y=77..540; the header is fixed after the first frame.
            // Release the unused carousel first, keeping peak RSS bounded.
            let mut pixels = vec![Pixel(0); W * H];
            for i in 0..=60 {
                let t = i * 1000 / 60;
                let texture = lab
                    .filtered
                    .as_ref()
                    .ok_or("cached reveal requires RGB888 source")?;
                if let Some(reveal) = &mut lab.reveal {
                    reveal.render(
                        t,
                        &lab.home,
                        lab.arcade.as_ref().unwrap(),
                        texture,
                        &mut pixels,
                    )?;
                } else if !render_arcade_card_filtered_into(
                    W,
                    H,
                    &lab.home,
                    lab.arcade.as_ref().unwrap(),
                    texture,
                    CARD,
                    t,
                    &mut pixels,
                ) {
                    return Err("cached reveal preparation failed".into());
                }
                lab.cache.push(pixels[77 * W..].to_vec());
            }
            lab.preparation
                .push(("animation_cache", stage.elapsed().as_millis() as u64));
            lab.reveal = None;
            lab.filtered = None;
            lab.cabinet.clear();
            lab.cabinet.shrink_to_fit();
        }
        Ok(lab)
    }
}
const REVEAL_SPLIT: usize = 311;
struct ParallelReveal {
    max_us: [u64; 3],
    tile: Option<Vec<Pixel>>,
    request: Option<std::sync::mpsc::Sender<(u32, Vec<Pixel>)>>,
    completed: std::sync::mpsc::Receiver<(u32, Vec<Pixel>, u64)>,
    worker: Option<std::thread::JoinHandle<()>>,
    storage_bytes: usize,
}
impl ParallelReveal {
    fn new(
        home: &[Pixel],
        arcade: &[Pixel],
        texture: &CabinetTexture,
        setup: Option<fn()>,
    ) -> Result<Self, String> {
        let home = home.to_vec();
        let arcade = arcade.to_vec();
        let texture = texture.clone();
        let tile = vec![Pixel(0); W * H];
        let storage_bytes = home.capacity() * 2
            + arcade.capacity() * 2
            + texture.storage_bytes()
            + tile.capacity() * 2;
        let (request, receive) = std::sync::mpsc::channel::<(u32, Vec<Pixel>)>();
        let (send, completed) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("mini-arcade-tile".into())
            .spawn(move || {
                if let Some(setup) = setup {
                    setup();
                }
                while let Ok((t, mut tile)) = receive.recv() {
                    let started = Instant::now();
                    if !render_arcade_card_filtered_band_into(
                        &home,
                        &arcade,
                        &texture,
                        t,
                        &mut tile,
                        (REVEAL_SPLIT, H),
                    ) {
                        break;
                    }
                    if send
                        .send((t, tile, started.elapsed().as_micros() as u64))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            max_us: [0; 3],
            tile: Some(tile),
            request: Some(request),
            completed,
            worker: Some(worker),
            storage_bytes,
        })
    }
    fn render(
        &mut self,
        t: u32,
        home: &[Pixel],
        arcade: &[Pixel],
        texture: &CabinetTexture,
        out: &mut [Pixel],
    ) -> Result<(), String> {
        self.request
            .as_ref()
            .ok_or("Arcade worker stopped")?
            .send((t, self.tile.take().ok_or("missing Arcade tile")?))
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        if !render_arcade_card_filtered_band_into(home, arcade, texture, t, out, (0, REVEAL_SPLIT))
        {
            return Err("invalid Arcade tile".into());
        }
        let primary_us = started.elapsed().as_micros() as u64;
        let waiting = Instant::now();
        let (completed, tile, secondary_us) = self.completed.recv().map_err(|e| e.to_string())?;
        let wait_us = waiting.elapsed().as_micros() as u64;
        self.max_us[0] = self.max_us[0].max(primary_us);
        self.max_us[1] = self.max_us[1].max(secondary_us);
        self.max_us[2] = self.max_us[2].max(wait_us);
        if completed != t {
            return Err("stale Arcade tile".into());
        }
        out[REVEAL_SPLIT * W..].copy_from_slice(&tile[REVEAL_SPLIT * W..]);
        self.tile = Some(tile);
        Ok(())
    }
}
impl Drop for ParallelReveal {
    fn drop(&mut self) {
        self.request.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Six rightward spins then six leftward spins, with a resting interval after
/// each spin. A following partial spin unwinds, exercising direction reversal.
fn browse(ms: u64) -> BrowseFrame {
    let t = ms % 8040;
    if t >= 7200 {
        let u = t - 7200;
        return BrowseFrame {
            selected: 0,
            target: 1,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: if u <= 420 { u as u32 } else { (840 - u) as u32 },
            duration_millis: 420,
        };
    }
    let left = t >= 3600;
    let step = (t % 3600) / 600;
    let u = t % 600;
    let selected = if left {
        (6 - step as usize) % 6
    } else {
        step as usize
    };
    let target = if left {
        (selected + 5) % 6
    } else {
        (selected + 1) % 6
    };
    if u >= 420 {
        BrowseFrame {
            selected: target,
            target,
            phase: BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        }
    } else {
        BrowseFrame {
            selected,
            target,
            phase: BrowsePhase::Flipping,
            direction: Some(if left {
                BrowseDirection::Left
            } else {
                BrowseDirection::Right
            }),
            progress_millis: u as u32,
            duration_millis: 420,
        }
    }
}
impl Effect for Lab {
    fn render(&mut self, elapsed: Duration, pixels: &mut [Pixel]) -> Result<Rect, String> {
        let ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        if let Some(arcade) = &self.arcade {
            let phase = ms % 2400;
            let t = if phase < 1000 {
                phase
            } else if phase < 1200 {
                1000
            } else {
                2200_u64.saturating_sub(phase)
            } as u32;
            if !self.cache.is_empty() {
                if t == 0 {
                    pixels.copy_from_slice(&self.home);
                } else {
                    let index = ((u64::from(t) * 60 + 500) / 1000).min(60) as usize;
                    pixels[..77 * W].copy_from_slice(&arcade[..77 * W]);
                    pixels[77 * W..].copy_from_slice(&self.cache[index]);
                }
                return Ok(full(W, H));
            }
            let ok = if let Some(reveal) = &mut self.reveal {
                reveal.render(
                    t,
                    &self.home,
                    arcade,
                    self.filtered.as_ref().unwrap(),
                    pixels,
                )?;
                true
            } else if let Some(texture) = &self.filtered {
                render_arcade_card_filtered_into(W, H, &self.home, arcade, texture, CARD, t, pixels)
            } else {
                render_arcade_card_transition_into(
                    W,
                    H,
                    &self.home,
                    arcade,
                    &self.cabinet,
                    CARD,
                    t,
                    pixels,
                )
            };
            if !ok {
                return Err("production Arcade renderer rejected fixture".into());
            }
            Ok(full(W, H))
        } else {
            let tiles = self
                .tiles
                .as_mut()
                .expect("card workload owns prepared tiles");
            let request = LauncherFrameRequest {
                frame: browse(ms),
                timestamp_us: ms * 1000,
                generation: ms,
            };
            tiles.render(request, self.launcher.as_mut().unwrap())?;
            pixels.copy_from_slice(self.launcher.as_ref().unwrap().pixels());
            Ok(Rect {
                x0: 296,
                y0: 120,
                x1: 934,
                y1: 495,
            })
        }
    }
    fn render_stage_max_us(&self) -> [u64; 3] {
        self.reveal.as_ref().map_or([0; 3], |r| r.max_us)
    }
    fn preparation_stages(&self) -> &[(&'static str, u64)] {
        &self.preparation
    }
    fn storage_bytes(&self) -> usize {
        self.reveal.as_ref().map_or(0, |r| r.storage_bytes)
            + self.tiles.as_ref().map_or(0, |t| t.storage_bytes)
            + self
                .launcher
                .as_ref()
                .map_or(0, PreparedLauncher::cached_raster_bytes)
            + self.cache.iter().map(|p| p.capacity() * 2).sum::<usize>()
            + self.home.capacity() * 2
            + self.cabinet.capacity() * 2
            + self.arcade.as_ref().map_or(0, |p| p.capacity() * 2)
            + self
                .filtered
                .as_ref()
                .map_or(0, CabinetTexture::storage_bytes)
    }
}

struct ParallelTiles {
    preparer: LauncherFramePreparer,
    left: PreparedLauncherFrame,
    right: Option<PreparedLauncherFrame>,
    request: Option<std::sync::mpsc::SyncSender<(LauncherFrameRequest, PreparedLauncherFrame)>>,
    completed: std::sync::mpsc::Receiver<PreparedLauncherFrame>,
    worker: Option<std::thread::JoinHandle<()>>,
    storage_bytes: usize,
}
impl ParallelTiles {
    fn new(preparer: LauncherFramePreparer, worker_setup: Option<fn()>) -> Result<Self, String> {
        let left = preparer.new_tile_buffer();
        let right = preparer.new_tile_buffer();
        let storage_bytes = left.storage_bytes() + right.storage_bytes();
        let (request, receive) =
            std::sync::mpsc::sync_channel::<(LauncherFrameRequest, PreparedLauncherFrame)>(1);
        let (send, completed) = std::sync::mpsc::sync_channel(1);
        let helper = preparer.clone();
        let worker = std::thread::Builder::new()
            .name("mini-card-tile".into())
            .spawn(move || {
                if let Some(setup) = worker_setup {
                    setup();
                }
                while let Ok((request, mut tile)) = receive.recv() {
                    helper.render_tile(request, &mut tile, (629, 934));
                    if send.send(tile).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            preparer,
            left,
            right: Some(right),
            request: Some(request),
            completed,
            worker: Some(worker),
            storage_bytes,
        })
    }
    fn render(
        &mut self,
        request: LauncherFrameRequest,
        launcher: &mut PreparedLauncher,
    ) -> Result<(), String> {
        self.request
            .as_ref()
            .ok_or("tile worker stopped")?
            .send((request, self.right.take().ok_or("missing tile buffer")?))
            .map_err(|e| e.to_string())?;
        self.preparer
            .render_tile(request, &mut self.left, (296, 629));
        let right = self.completed.recv().map_err(|e| e.to_string())?;
        if right.request() != Some(request) {
            return Err("stale card tile completion".into());
        }
        launcher.compose_tiles(&self.left, &right);
        self.right = Some(right);
        Ok(())
    }
}
impl Drop for ParallelTiles {
    fn drop(&mut self) {
        self.request.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_tiles_match_complete_production_frames() {
        for preset in [Preset::Default, Preset::Dithered, Preset::Rgb888] {
            let mut lab = Lab::new("launcher-cards", preset, None).unwrap();
            let quality = match preset {
                Preset::Default => CardRenderQuality::Current,
                Preset::Dithered => CardRenderQuality::Dithered,
                _ => CardRenderQuality::Rgb888,
            };
            let mut reference = crate::fixture::prepare_quality(W, H, quality);
            let mut pixels = vec![Pixel(0); W * H];
            for ms in [0, 105, 210, 419, 420, 3810, 7410, 7830, 8040] {
                lab.render(Duration::from_millis(ms), &mut pixels).unwrap();
                reference.render_frame(browse(ms));
                assert_eq!(pixels, reference.pixels(), "{preset:?} at {ms}");
            }
        }
    }
    #[test]
    fn storyboard_visits_every_card_both_directions_and_unwinds() {
        for i in 0..6 {
            assert_eq!(browse(i * 600).selected, i as usize);
            assert_eq!(browse(3600 + i * 600).selected, (6 - i as usize) % 6);
        }
        assert_eq!(browse(7210), browse(8030));
        assert_eq!(browse(419).phase, BrowsePhase::Flipping);
        assert_eq!(browse(420).phase, BrowsePhase::Settled);
    }
    #[test]
    fn arcade_endpoints_match_and_reset_is_repeatable_for_all_modes() {
        for preset in [Preset::Default, Preset::Dithered, Preset::Rgb888] {
            let mut lab = Lab::new("arcade-transition", preset, None).unwrap();
            let mut pixels = vec![Pixel(0); W * H];
            lab.render(Duration::ZERO, &mut pixels).unwrap();
            assert_eq!(pixels, lab.home);
            lab.render(Duration::from_millis(1000), &mut pixels)
                .unwrap();
            assert_eq!(Some(&pixels), lab.arcade.as_ref());
            if preset != Preset::Default {
                lab.render(Duration::from_millis(999), &mut pixels).unwrap();
                let destination = lab.arcade.as_ref().unwrap();
                for y in 77..500 {
                    for x in 490..960 {
                        if (572..892).contains(&x) && (96..416).contains(&y) {
                            continue;
                        }
                        assert_eq!(
                            pixels[y * W + x],
                            destination[y * W + x],
                            "cabinet must not change quality on settlement"
                        );
                    }
                }
            }
            lab.render(Duration::from_millis(1700), &mut pixels)
                .unwrap();
            let reverse = pixels.clone();
            lab.render(Duration::from_millis(500), &mut pixels).unwrap();
            assert_eq!(pixels, reverse);
            lab.render(Duration::from_millis(2400), &mut pixels)
                .unwrap();
            assert_eq!(pixels, lab.home);
        }
    }
    #[test]
    fn cached_reveal_matches_filtered_samples_and_has_a_bounded_budget() {
        let mut direct = Lab::new("arcade-transition", Preset::Rgb888, None).unwrap();
        let mut cached = Lab::new("arcade-transition", Preset::Cached, None).unwrap();
        assert_eq!(cached.cache.len(), 61);
        assert!(cached.storage_bytes() < 60 * 1024 * 1024);
        let mut expected = vec![Pixel(0); W * H];
        let mut actual = expected.clone();
        for i in [0, 1, 15, 30, 45, 59, 60] {
            let t = i * 1000 / 60;
            direct
                .render(Duration::from_millis(t), &mut expected)
                .unwrap();
            cached
                .render(Duration::from_millis(t), &mut actual)
                .unwrap();
            assert_eq!(actual, expected, "cached pose {i}");
        }
        cached
            .render(Duration::from_millis(1700), &mut actual)
            .unwrap();
        direct
            .render(Duration::from_millis(500), &mut expected)
            .unwrap();
        assert_eq!(actual, expected);
    }
    #[test]
    fn scanlines_match_reference_through_mip_changes_edges_and_reverse() {
        let mut reference = Lab::new("arcade-transition", Preset::Rgb888, None).unwrap();
        let mut scanline = Lab::new("arcade-transition", Preset::Scanline, None).unwrap();
        assert!(scanline.cache.is_empty());
        assert!(scanline.storage_bytes() - reference.storage_bytes() < 6 * 1024 * 1024);
        let mut expected = vec![Pixel(0); W * H];
        let mut actual = expected.clone();
        for ms in [
            0, 1, 79, 80, 121, 219, 301, 379, 420, 480, 499, 500, 501, 599, 639, 760, 839, 840,
            919, 999, 1000, 1201, 1700, 2199, 2200,
        ] {
            reference
                .render(Duration::from_millis(ms), &mut expected)
                .unwrap();
            scanline
                .render(Duration::from_millis(ms), &mut actual)
                .unwrap();
            assert_eq!(actual, expected, "scanline pose {ms}");
        }
    }
    #[test]
    fn accelerated_runtime_cache_matches_reference_and_releases_workers() {
        let mut fast = Lab::new("arcade-transition", Preset::CachedFast, None).unwrap();
        assert_eq!(fast.cache.len(), 61);
        assert!(fast.reveal.is_none() && fast.filtered.is_none());
        assert!(fast.storage_bytes() < 60 * 1024 * 1024);
        let mut reference = Lab::new("arcade-transition", Preset::Rgb888, None).unwrap();
        let mut actual = vec![Pixel(0); W * H];
        let mut expected = actual.clone();
        for i in [0, 1, 7, 15, 30, 45, 59, 60] {
            let t = i * 1000 / 60;
            fast.render(Duration::from_millis(t), &mut actual).unwrap();
            reference
                .render(Duration::from_millis(t), &mut expected)
                .unwrap();
            assert_eq!(actual, expected, "runtime cached pose {i}");
        }
    }
    #[test]
    fn geometry_and_preset_mismatches_are_rejected() {
        assert!(crate::Scene::new("launcher-cards", Preset::Default, 960, 600).is_err());
        assert!(crate::Scene::new("launcher-cards", Preset::Reduced, W, H).is_err());
        assert!(crate::Scene::new("diagnostic", Preset::Rgb888, W, H).is_err());
        assert!(crate::Scene::new("launcher-cards", Preset::Scanline, W, H).is_err());
    }
}
