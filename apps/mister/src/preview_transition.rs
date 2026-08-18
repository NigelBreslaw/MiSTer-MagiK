// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral screenshot transition timing and RGB565 composition.

use crate::framebuffer::target::DirtyRect;
use crate::visual_composition::{PreviewFrame, PreviewSurface, compose_preview_frame};
use mister_magik_framebuffer_scenes::{
    blend_rgb565_black_neon_if_available, blend_rgb565_neon_if_available,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
struct ActiveTransition<E> {
    transition_id: u64,
    effect: E,
    start_elapsed: Duration,
    duration: Duration,
    last_retarget_elapsed: Duration,
    completed_presented: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewTransitionState<E> {
    pub effect: E,
    pub progress: f32,
    pub active: bool,
}

pub struct PreviewTransitionController<E> {
    last_transition_id: u64,
    active: Option<ActiveTransition<E>>,
}

impl<E> Default for PreviewTransitionController<E> {
    fn default() -> Self {
        Self {
            last_transition_id: u64::MAX,
            active: None,
        }
    }
}

impl<E: Copy> PreviewTransitionController<E> {
    pub fn reset(&mut self) {
        self.last_transition_id = u64::MAX;
        self.active = None;
    }

    pub fn update(
        &mut self,
        transition_id: Option<u64>,
        has_previous: bool,
        scheduled_effect: E,
        duration: Duration,
        elapsed: Duration,
    ) -> PreviewTransitionState<E> {
        let Some(transition_id) = transition_id else {
            self.active = None;
            return PreviewTransitionState {
                effect: scheduled_effect,
                progress: 1.0,
                active: false,
            };
        };
        if transition_id != self.last_transition_id {
            self.last_transition_id = transition_id;
            self.active = if has_previous {
                let (effect, start_elapsed, duration) = self
                    .active
                    .filter(|active| {
                        elapsed.saturating_sub(active.last_retarget_elapsed) < active.duration
                    })
                    .map(|active| (active.effect, active.start_elapsed, active.duration))
                    .unwrap_or((scheduled_effect, elapsed, duration));
                Some(ActiveTransition {
                    transition_id,
                    effect,
                    start_elapsed,
                    duration,
                    last_retarget_elapsed: elapsed,
                    completed_presented: false,
                })
            } else {
                None
            };
        }
        if let Some(active) = self.active {
            if active.transition_id == transition_id {
                let progress = transition_progress(
                    elapsed.saturating_sub(active.start_elapsed),
                    active.duration,
                );
                if progress < 1.0 {
                    return PreviewTransitionState {
                        effect: active.effect,
                        progress,
                        active: true,
                    };
                }
                let needs_final_present = !active.completed_presented;
                if let Some(active) = self.active.as_mut() {
                    active.completed_presented = true;
                }
                return PreviewTransitionState {
                    effect: active.effect,
                    progress: 1.0,
                    active: needs_final_present,
                };
            }
            self.active = None;
        }
        PreviewTransitionState {
            effect: scheduled_effect,
            progress: 1.0,
            active: false,
        }
    }
}

pub fn transition_duration(duration: Duration, divisor: u32) -> Duration {
    let divisor = divisor.max(1) as u128;
    let micros = (duration.as_micros() / divisor).max(1);
    Duration::from_micros(micros.min(u64::MAX as u128) as u64)
}

pub fn transition_duration_ratio(duration: Duration, numerator: u32, denominator: u32) -> Duration {
    let numerator = numerator.max(1) as u128;
    let denominator = denominator.max(1) as u128;
    let micros = (duration.as_micros().saturating_mul(numerator) / denominator).max(1);
    Duration::from_micros(micros.min(u64::MAX as u128) as u64)
}

fn transition_progress(elapsed: Duration, duration: Duration) -> f32 {
    let denominator = duration.as_secs_f32();
    if denominator <= 0.0 {
        return 1.0;
    }
    (elapsed.as_secs_f32() / denominator).clamp(0.0, 1.0)
}

#[inline(always)]
pub fn blend_rgb565_bucket(from: Rgb565Pixel, to: Rgb565Pixel, alpha_bucket: u16) -> Rgb565Pixel {
    macro_rules! bucket {
        ($alpha:literal, $inverse:literal) => {
            blend_rgb565_const::<$alpha, $inverse>(from, to)
        };
    }
    match alpha_bucket.min(32) {
        0 => from,
        1 => bucket!(1, 31),
        2 => bucket!(2, 30),
        3 => bucket!(3, 29),
        4 => bucket!(4, 28),
        5 => bucket!(5, 27),
        6 => bucket!(6, 26),
        7 => bucket!(7, 25),
        8 => bucket!(8, 24),
        9 => bucket!(9, 23),
        10 => bucket!(10, 22),
        11 => bucket!(11, 21),
        12 => bucket!(12, 20),
        13 => bucket!(13, 19),
        14 => bucket!(14, 18),
        15 => bucket!(15, 17),
        16 => bucket!(16, 16),
        17 => bucket!(17, 15),
        18 => bucket!(18, 14),
        19 => bucket!(19, 13),
        20 => bucket!(20, 12),
        21 => bucket!(21, 11),
        22 => bucket!(22, 10),
        23 => bucket!(23, 9),
        24 => bucket!(24, 8),
        25 => bucket!(25, 7),
        26 => bucket!(26, 6),
        27 => bucket!(27, 5),
        28 => bucket!(28, 4),
        29 => bucket!(29, 3),
        30 => bucket!(30, 2),
        31 => bucket!(31, 1),
        _ => to,
    }
}

#[inline(always)]
fn blend_rgb565_const<const ALPHA: u32, const INVERSE: u32>(
    from: Rgb565Pixel,
    to: Rgb565Pixel,
) -> Rgb565Pixel {
    let from = u32::from(from.0);
    let to = u32::from(to.0);
    let red_blue = (((from & 0xf81f) * INVERSE + (to & 0xf81f) * ALPHA) >> 5) & 0xf81f;
    let green = (((from & 0x07e0) * INVERSE + (to & 0x07e0) * ALPHA) >> 5) & 0x07e0;
    Rgb565Pixel((red_blue | green) as u16)
}

pub fn blend_rgb565_rows_bucketed(
    destination: &mut [Rgb565Pixel],
    previous: &[Rgb565Pixel],
    current: &[Rgb565Pixel],
    alpha_bucket: u16,
) {
    debug_assert!(previous.len() >= destination.len());
    debug_assert!(current.len() >= destination.len());
    let length = destination.len();
    let alpha = alpha_bucket.min(32);
    if alpha == 0 {
        destination.copy_from_slice(&previous[..length]);
        return;
    }
    if alpha >= 32 {
        destination.copy_from_slice(&current[..length]);
        return;
    }
    if blend_rgb565_neon_if_available(destination, previous, current, 0, length, alpha) {
        return;
    }
    for index in 0..destination.len() {
        destination[index] = blend_rgb565_bucket(previous[index], current[index], alpha);
    }
}

pub fn blend_rgb565_row_with_black(
    destination: &mut [Rgb565Pixel],
    pixels: &[Rgb565Pixel],
    alpha_bucket: u16,
    fade_in: bool,
) {
    debug_assert!(pixels.len() >= destination.len());
    let black = Rgb565Pixel(0);
    let length = destination.len();
    let alpha = alpha_bucket.min(32);
    if alpha == 0 {
        if fade_in {
            destination.fill(black);
        } else {
            destination.copy_from_slice(&pixels[..length]);
        }
        return;
    }
    if alpha >= 32 {
        if fade_in {
            destination.copy_from_slice(&pixels[..length]);
        } else {
            destination.fill(black);
        }
        return;
    }
    if blend_rgb565_black_neon_if_available(destination, pixels, 0, length, alpha, fade_in) {
        return;
    }
    for index in 0..destination.len() {
        destination[index] = if fade_in {
            blend_rgb565_bucket(black, pixels[index], alpha)
        } else {
            blend_rgb565_bucket(pixels[index], black, alpha)
        };
    }
}

pub struct Rgb565PreviewTransitionCompositor {
    previous: Vec<Rgb565Pixel>,
    current: Vec<Rgb565Pixel>,
}

impl Rgb565PreviewTransitionCompositor {
    pub fn new(frame_width: usize, frame_height: usize) -> Self {
        let length = frame_width.saturating_mul(frame_height);
        Self {
            previous: vec![Rgb565Pixel(0); length],
            current: vec![Rgb565Pixel(0); length],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn compose(
        &mut self,
        destination: &mut [Rgb565Pixel],
        frame_width: usize,
        frame_height: usize,
        screen: DirtyRect,
        previous: Option<PreviewFrame<'_>>,
        current: PreviewFrame<'_>,
        progress: f32,
        surface: PreviewSurface,
    ) -> Option<DirtyRect> {
        let length = frame_width.saturating_mul(frame_height);
        if destination.len() < length {
            return None;
        }
        self.previous.resize(length, Rgb565Pixel(0));
        self.current.resize(length, Rgb565Pixel(0));
        self.previous.fill(Rgb565Pixel(0));
        self.current.fill(Rgb565Pixel(0));
        if let Some(previous) = previous {
            compose_preview_frame(
                &mut self.previous,
                frame_width,
                frame_height,
                screen,
                previous,
                true,
                PreviewSurface::full(frame_width),
            );
        }
        compose_preview_frame(
            &mut self.current,
            frame_width,
            frame_height,
            screen,
            current,
            true,
            PreviewSurface::full(frame_width),
        );
        let alpha = ((progress.clamp(0.0, 1.0) * 255.0).round() as u16 + 4) >> 3;
        for y in screen.y0..screen.y1.min(frame_height) {
            let x0 = screen.x0.min(frame_width);
            let x1 = screen.x1.min(frame_width);
            let source = y * frame_width + x0..y * frame_width + x1;
            let destination_start = surface.row_start(y, x0);
            let destination_end = destination_start + (x1 - x0);
            blend_rgb565_rows_bucketed(
                &mut destination[destination_start..destination_end],
                &self.previous[source.clone()],
                &self.current[source],
                alpha,
            );
        }
        Some(screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retarget_keeps_the_original_transition_deadline() {
        let mut controller = PreviewTransitionController::default();
        let duration = Duration::from_millis(200);
        let first = controller.update(Some(1), true, (), duration, Duration::ZERO);
        assert_eq!(first.progress, 0.0);
        let retarget = controller.update(Some(2), true, (), duration, Duration::from_millis(50));
        assert!((retarget.progress - 0.25).abs() < 0.001);
        let complete = controller.update(Some(2), true, (), duration, Duration::from_millis(200));
        assert_eq!(complete.progress, 1.0);
        assert!(complete.active);
    }

    #[test]
    fn rgb565_bucket_endpoints_are_exact() {
        let red = Rgb565Pixel(0xf800);
        let blue = Rgb565Pixel(0x001f);
        assert_eq!(blend_rgb565_bucket(red, blue, 0), red);
        assert_eq!(blend_rgb565_bucket(red, blue, 32), blue);
    }
}
