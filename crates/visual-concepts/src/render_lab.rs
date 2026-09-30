// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Mini exercises production cards, Arcade and Settings cog rendering.
use super::{Effect, Pixel, Preset, Rect, full};
use mister_magik_framebuffer_scenes::{
    arcade_card::{ArcadeCardRenderer, CabinetTexture},
    launcher::{
        LauncherFramePreparer, LauncherFrameRequest, PreparedLauncher, PreparedLauncherFrame,
    },
    launcher_navigation::{BrowseDirection, BrowseFrame, BrowsePhase},
    settings_cog::{CogTexture, render_settings_cog_transition_into},
};
use std::{
    io::Cursor,
    sync::Arc,
    time::{Duration, Instant},
};
const W: usize = 960;
const H: usize = 540;
pub(super) struct Lab {
    first_frame: bool,
    held_navigation: bool,
    launcher: Option<PreparedLauncher>,
    home: Vec<Pixel>,
    tiles: Option<ParallelTiles>,
    reveal: Option<ArcadeCardRenderer>,
    cog: Option<(CogTexture, Vec<Pixel>)>,
    cog_last_us: u64,
    cog_max_us: u64,
    preparation: Vec<(&'static str, u64)>,
}
impl Lab {
    pub(super) fn new(
        name: &str,
        preset: Preset,
        worker_setup: Option<fn()>,
    ) -> Result<Self, String> {
        if preset == Preset::Reduced {
            return Err("reduced is not a renderer workload".into());
        }
        let mut preparation = Vec::new();
        let stage = Instant::now();
        let (launcher, home, cached) = if matches!(name, "launcher-cards" | "launcher-cards-held") {
            let launcher = crate::fixture::prepare(W, H);
            let home = launcher.pixels().to_vec();
            (Some(launcher), home, false)
        } else if name == "settings-transition" {
            let launcher = crate::fixture::prepare_selected(W, H, 5);
            (None, launcher.pixels().to_vec(), false)
        } else {
            let (home, cached) = crate::fixture::home_prepared();
            (None, home, cached)
        };
        preparation.push((
            if cached {
                "launcher_snapshot"
            } else {
                "launcher_fixture_cold"
            },
            stage.elapsed().as_millis() as u64,
        ));
        let reveal = if name == "arcade-transition" {
            let stage = Instant::now();
            let mut frame = destination_snapshot(include_bytes!(
                "../../../apps/mister/tests/visual-baselines/launcher/hdmi-arcade.png"
            ))?;
            preparation.push(("destination_decode", stage.elapsed().as_millis() as u64));
            let stage = Instant::now();
            let texture = CabinetTexture::from_rgb888(include_bytes!(
                "../../../apps/mister/assets/ui/arcade/cabinet-483x519.rgb888"
            ))?;
            if !texture.prepare_destination(&mut frame) {
                return Err("invalid resting cabinet".into());
            }
            preparation.push(("cabinet_texture", stage.elapsed().as_millis() as u64));
            let stage = Instant::now();
            let reveal = ArcadeCardRenderer::new(
                Arc::new(home.clone()),
                Arc::new(frame),
                &texture,
                worker_setup,
            )?;
            preparation.push(("worker_setup", stage.elapsed().as_millis() as u64));
            Some(reveal)
        } else {
            None
        };
        let cog = if name == "settings-transition" {
            let stage = Instant::now();
            let mut destination = destination_snapshot(include_bytes!(
                "../../../apps/mister/tests/visual-baselines/launcher/hdmi-settings.png"
            ))?;
            preparation.push(("destination_decode", stage.elapsed().as_millis() as u64));
            let stage = Instant::now();
            let texture = CogTexture::from_rgb888(include_bytes!(
                "../../../apps/mister/assets/ui/settings/cog-backdrop-412x374.rgb888"
            ))?;
            if !texture.prepare_destination(&mut destination) {
                return Err("invalid resting cog".into());
            }
            preparation.push(("cog_texture", stage.elapsed().as_millis() as u64));
            Some((texture, destination))
        } else {
            None
        };
        let stage = Instant::now();
        let tiles = launcher
            .as_ref()
            .map(|launcher| ParallelTiles::new(launcher.frame_preparer(), worker_setup))
            .transpose()?;
        if tiles.is_some() {
            preparation.push(("worker_setup", stage.elapsed().as_millis() as u64));
        }
        Ok(Self {
            first_frame: true,
            held_navigation: name == "launcher-cards-held",
            launcher,
            home,
            tiles,
            reveal,
            cog,
            cog_last_us: 0,
            cog_max_us: 0,
            preparation,
        })
    }
}
fn destination_snapshot(source: &[u8]) -> Result<Vec<Pixel>, String> {
    let mut decoder = png::Decoder::new(Cursor::new(source));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut bytes = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("unbounded destination snapshot")?
    ];
    let info = reader.next_frame(&mut bytes).map_err(|e| e.to_string())?;
    if (info.width, info.height) != (W as u32, H as u32) || info.bit_depth != png::BitDepth::Eight {
        return Err("destination snapshot geometry or bit depth changed".into());
    }
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return Err("destination snapshot must be RGB or RGBA".into()),
    };
    Ok(bytes[..info.buffer_size()]
        .chunks_exact(channels)
        .map(|p| {
            Pixel((u16::from(p[0]) >> 3) << 11 | (u16::from(p[1]) >> 2) << 5 | u16::from(p[2]) >> 3)
        })
        .collect())
}
fn reveal_time(ms: u64) -> u32 {
    let phase = ms % 2400;
    (if phase < 1000 {
        phase
    } else if phase < 1200 {
        1000
    } else {
        2200_u64.saturating_sub(phase)
    }) as u32
}
// Root carousel default cruise speed: 360 px/s * 0.7 / 36 px per card.
// Feed position units directly, avoiding a spring restart at every card boundary.
fn browse_held(ms: u64) -> BrowseFrame {
    use mister_magik_framebuffer_scenes::launcher_navigation::SPRING_POSITION_UNITS;
    let position = (u128::from(ms) * 7 * u128::from(SPRING_POSITION_UNITS) / 1000)
        % (6 * u128::from(SPRING_POSITION_UNITS));
    let selected = (position / u128::from(SPRING_POSITION_UNITS)) as usize;
    BrowseFrame {
        selected,
        target: (selected + 1) % 6,
        phase: BrowsePhase::Flipping,
        direction: Some(BrowseDirection::Right),
        progress_millis: (position % u128::from(SPRING_POSITION_UNITS)) as u32,
        duration_millis: SPRING_POSITION_UNITS,
    }
}

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
    fn reset(&mut self) {
        self.first_frame = true;
    }
    fn render(&mut self, elapsed: Duration, pixels: &mut [Pixel]) -> Result<Rect, String> {
        let ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        if let Some((texture, destination)) = &self.cog {
            let started = Instant::now();
            if !render_settings_cog_transition_into(
                &self.home,
                destination,
                texture,
                reveal_time(ms),
                pixels,
            ) {
                return Err("invalid cog frame".into());
            }
            self.cog_last_us = started.elapsed().as_micros() as u64;
            self.cog_max_us = self.cog_max_us.max(self.cog_last_us);
            Ok(full(W, H))
        } else if let Some(reveal) = &mut self.reveal {
            let t = reveal_time(ms);
            reveal.render(t, pixels)?;
            Ok(full(W, H))
        } else {
            if self.first_frame {
                pixels.copy_from_slice(&self.home);
                self.first_frame = false;
            }
            self.tiles.as_mut().unwrap().render(
                LauncherFrameRequest {
                    frame: if self.held_navigation {
                        browse_held(ms)
                    } else {
                        browse(ms)
                    },
                    timestamp_us: ms * 1000,
                    generation: ms,
                },
                pixels,
            )?;
            Ok(Rect {
                x0: 296,
                y0: 120,
                x1: 934,
                y1: 495,
            })
        }
    }
    fn render_stage_last_us(&self) -> [u64; 3] {
        if self.cog.is_some() {
            return [self.cog_last_us, 0, 0];
        }
        self.reveal.as_ref().map_or_else(
            || self.tiles.as_ref().map_or([0; 3], |t| t.last_us),
            ArcadeCardRenderer::last_us,
        )
    }
    fn render_stage_max_us(&self) -> [u64; 3] {
        if self.cog.is_some() {
            return [self.cog_max_us, 0, 0];
        }
        self.reveal.as_ref().map_or_else(
            || self.tiles.as_ref().map_or([0; 3], |t| t.max_us),
            ArcadeCardRenderer::max_us,
        )
    }
    fn preparation_stages(&self) -> &[(&'static str, u64)] {
        &self.preparation
    }
    fn storage_bytes(&self) -> usize {
        self.cog.as_ref().map_or(0, |(texture, destination)| {
            texture.storage_bytes() + destination.capacity() * 2
        }) + self
            .reveal
            .as_ref()
            .map_or(0, ArcadeCardRenderer::storage_bytes)
            + self.tiles.as_ref().map_or(0, |t| t.storage_bytes)
            + self
                .launcher
                .as_ref()
                .map_or(0, PreparedLauncher::cached_raster_bytes)
            + self.home.capacity() * 2
    }
}
struct ParallelTiles {
    preparer: LauncherFramePreparer,
    left: PreparedLauncherFrame,
    right: Option<PreparedLauncherFrame>,
    request: Option<std::sync::mpsc::SyncSender<(LauncherFrameRequest, PreparedLauncherFrame)>>,
    completed: std::sync::mpsc::Receiver<(PreparedLauncherFrame, u64)>,
    max_us: [u64; 3],
    last_us: [u64; 3],
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
                    let started = Instant::now();
                    helper.render_tile(request, &mut tile, (629, 934));
                    if send
                        .send((tile, started.elapsed().as_micros() as u64))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            preparer,
            max_us: [0; 3],
            last_us: [0; 3],
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
        pixels: &mut [Pixel],
    ) -> Result<(), String> {
        self.request
            .as_ref()
            .ok_or("tile worker stopped")?
            .send((request, self.right.take().ok_or("missing tile buffer")?))
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        self.preparer
            .render_tile_into(request, &mut self.left, pixels, (296, 629), false);
        let primary_us = started.elapsed().as_micros() as u64;
        self.max_us[0] = self.max_us[0].max(primary_us);
        let waiting = Instant::now();
        let (right, secondary_us) = self.completed.recv().map_err(|e| e.to_string())?;
        self.max_us[1] = self.max_us[1].max(secondary_us);
        let wait_us = waiting.elapsed().as_micros() as u64;
        self.last_us = [primary_us, secondary_us, wait_us];
        self.max_us[2] = self.max_us[2].max(wait_us);
        if right.request() != Some(request) {
            return Err("stale card tile completion".into());
        }
        // The primary tile already owns this cached-memory destination. Merge
        // only the helper band, then the presenter transfers the completed frame.
        for y in 120..495 {
            pixels[y * W + 629..y * W + 934]
                .copy_from_slice(&right.pixels()[y * W + 629..y * W + 934]);
        }
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
    fn held_cards_keep_flipping_across_slot_and_wrap_boundaries() {
        use mister_magik_framebuffer_scenes::launcher_navigation::SPRING_POSITION_UNITS;
        for ms in 0..8_000 {
            let frame = browse_held(ms);
            assert_eq!(frame.phase, BrowsePhase::Flipping);
            assert_eq!(frame.direction, Some(BrowseDirection::Right));
            assert_eq!(frame.duration_millis, SPRING_POSITION_UNITS);
            assert_eq!(frame.target, (frame.selected + 1) % 6);
        }
        assert_eq!(browse_held(1_000).selected, 1);
        assert_eq!(browse_held(6_000).selected, 0);
        let before = browse_held(142);
        let after = browse_held(143);
        assert_eq!((before.selected, after.selected), (0, 1));
        assert!(before.progress_millis > SPRING_POSITION_UNITS * 99 / 100);
        assert!(after.progress_millis < SPRING_POSITION_UNITS / 100);
    }

    #[test]
    fn parallel_tiles_match_complete_production_frames() {
        let mut lab = Lab::new("launcher-cards", Preset::Default, None).unwrap();
        let mut reference = crate::fixture::prepare(W, H);
        let mut pixels = vec![Pixel(0); W * H];
        for ms in [0, 105, 210, 419, 420, 3810, 7410, 7830, 8040] {
            lab.render(Duration::from_millis(ms), &mut pixels).unwrap();
            reference.render_frame(browse(ms));
            assert_eq!(pixels, reference.pixels(), "{ms}");
        }
    }
    #[test]
    fn settings_reset_reverse_and_settlement_use_the_live_cog_renderer() {
        let mut lab = Lab::new("settings-transition", Preset::Default, None).unwrap();
        let mut pixels = vec![Pixel(0); W * H];
        lab.render(Duration::ZERO, &mut pixels).unwrap();
        assert_eq!(pixels, lab.home);
        lab.render(Duration::from_millis(500), &mut pixels).unwrap();
        let midpoint = pixels.clone();
        lab.render(Duration::from_millis(1700), &mut pixels)
            .unwrap();
        assert_eq!(pixels, midpoint);
        lab.render(Duration::from_millis(1000), &mut pixels)
            .unwrap();
        assert_eq!(pixels, lab.cog.as_ref().unwrap().1);
        lab.reset();
        lab.render(Duration::ZERO, &mut pixels).unwrap();
        assert_eq!(pixels, lab.home);
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
    fn arcade_reset_and_reverse_reuse_the_live_renderer() {
        let mut lab = Lab::new("arcade-transition", Preset::Default, None).unwrap();
        let mut pixels = vec![Pixel(0); W * H];
        lab.render(Duration::ZERO, &mut pixels).unwrap();
        assert_eq!(pixels, lab.home);
        lab.render(Duration::from_millis(500), &mut pixels).unwrap();
        let opening = pixels.clone();
        lab.render(Duration::from_millis(1700), &mut pixels)
            .unwrap();
        assert_eq!(pixels, opening);
        lab.render(Duration::from_millis(2400), &mut pixels)
            .unwrap();
        assert_eq!(pixels, lab.home);
    }
    #[test]
    fn geometry_and_preset_mismatches_are_rejected() {
        assert!(crate::Scene::new("launcher-cards", Preset::Default, 960, 600).is_err());
        assert!(crate::Scene::new("launcher-cards", Preset::Reduced, W, H).is_err());
        assert!(crate::Scene::new("diagnostic", Preset::Rgb888, W, H).is_err());
        assert!(crate::Scene::new("launcher-cards", Preset::Scanline, W, H).is_err());
    }
}
