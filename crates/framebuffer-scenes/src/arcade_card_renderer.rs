// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded live row renderer. The application owns thread placement and snapshots.
use super::{CabinetTexture, render_arcade_card_band_into};
use crate::Rgb565Pixel as Pixel;
use std::{sync::Arc, time::Instant};
const W: usize = 960;
const H: usize = 540;
const REVEAL_SPLIT: usize = 311;
pub struct ArcadeCardRenderer {
    home: Arc<Vec<Pixel>>,
    arcade: Arc<Vec<Pixel>>,
    texture: CabinetTexture,
    max_us: [u64; 3],
    last_us: [u64; 3],
    tile: Option<Vec<Pixel>>,
    request: Option<std::sync::mpsc::SyncSender<(u32, Vec<Pixel>)>>,
    completed: std::sync::mpsc::Receiver<(u32, Vec<Pixel>, u64)>,
    worker: Option<std::thread::JoinHandle<()>>,
    storage_bytes: usize,
}
impl ArcadeCardRenderer {
    pub fn new(
        home: Arc<Vec<Pixel>>,
        arcade: Arc<Vec<Pixel>>,
        texture: &CabinetTexture,
        setup: Option<fn()>,
    ) -> Result<Self, String> {
        if home.len() != W * H || arcade.len() != W * H {
            return Err("invalid Arcade snapshot geometry".into());
        }
        let primary_home = Arc::clone(&home);
        let primary_arcade = Arc::clone(&arcade);
        let primary_texture = texture.clone();
        let texture = texture.clone();
        let tile = vec![Pixel(0); W * H];
        let storage_bytes = home.capacity() * 2
            + arcade.capacity() * 2
            + texture.storage_bytes()
            + tile.capacity() * 2;
        let (request, receive) = std::sync::mpsc::sync_channel::<(u32, Vec<Pixel>)>(1);
        let (send, completed) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("arcade-card-tile".into())
            .spawn(move || {
                if let Some(setup) = setup {
                    setup();
                }
                while let Ok((t, mut tile)) = receive.recv() {
                    let started = Instant::now();
                    if !render_arcade_card_band_into(
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
            home: primary_home,
            arcade: primary_arcade,
            texture: primary_texture,
            max_us: [0; 3],
            last_us: [0; 3],
            tile: Some(tile),
            request: Some(request),
            completed,
            worker: Some(worker),
            storage_bytes,
        })
    }
    pub fn render(&mut self, t: u32, out: &mut [Pixel]) -> Result<(), String> {
        self.request
            .as_ref()
            .ok_or("Arcade worker stopped")?
            .send((t, self.tile.take().ok_or("missing Arcade tile")?))
            .map_err(|e| e.to_string())?;
        let started = Instant::now();
        if !render_arcade_card_band_into(
            &self.home,
            &self.arcade,
            &self.texture,
            t,
            out,
            (0, REVEAL_SPLIT),
        ) {
            return Err("invalid Arcade tile".into());
        }
        let primary_us = started.elapsed().as_micros() as u64;
        let waiting = Instant::now();
        let (completed, tile, secondary_us) = self.completed.recv().map_err(|e| e.to_string())?;
        let wait_us = waiting.elapsed().as_micros() as u64;
        self.last_us = [primary_us, secondary_us, wait_us];
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
impl Drop for ArcadeCardRenderer {
    fn drop(&mut self) {
        self.request.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl ArcadeCardRenderer {
    pub fn last_us(&self) -> [u64; 3] {
        self.last_us
    }
    pub fn max_us(&self) -> [u64; 3] {
        self.max_us
    }
    pub fn storage_bytes(&self) -> usize {
        self.storage_bytes
    }
}
impl std::fmt::Debug for ArcadeCardRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArcadeCardRenderer").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_reveal_matches_serial_through_fades_identity_and_reverse() {
        let texture = CabinetTexture::from_rgb888(&vec![
            97;
            super::super::CABINET_WIDTH
                * super::super::CABINET_HEIGHT
                * 3
        ])
        .unwrap();
        let home = Arc::new(vec![Pixel(0x1234); W * H]);
        let mut arcade = vec![Pixel(0x5a6d); W * H];
        texture.prepare_destination(&mut arcade);
        let arcade = Arc::new(arcade);
        let mut renderer =
            ArcadeCardRenderer::new(Arc::clone(&home), Arc::clone(&arcade), &texture, None)
                .unwrap();
        let mut actual = vec![Pixel(0); W * H];
        let mut expected = actual.clone();
        for t in [
            0, 1, 80, 120, 219, 340, 360, 379, 499, 500, 640, 760, 799, 839, 840, 919, 920, 999,
            1000, 500, 0,
        ] {
            renderer.render(t, &mut actual).unwrap();
            assert!(super::super::render_arcade_card_into(
                W,
                H,
                &home,
                &arcade,
                &texture,
                super::super::HDMI_CARD,
                t,
                &mut expected
            ));
            assert_eq!(actual, expected, "parallel bands at {t}");
        }
        drop(renderer);
        assert!(ArcadeCardRenderer::new(Arc::new(vec![]), arcade, &texture, None).is_err());
    }
}
