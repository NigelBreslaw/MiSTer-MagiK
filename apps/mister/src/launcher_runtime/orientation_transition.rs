// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral RGB565 monitor-orientation transition compositor.

use crate::settings::ScreenOrientation;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

pub const ORIENTATION_QUARTER_TURN_DURATION: Duration = Duration::from_millis(300);
pub const ORIENTATION_OPPOSITE_TURN_DURATION: Duration = Duration::from_millis(450);
const DESTINATION_CROSSFADE_START: f32 = 0.72;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrientationTransitionCompletion {
    pub from: ScreenOrientation,
    pub to: ScreenOrientation,
}

pub struct OrientationTransitionRuntime {
    width: usize,
    height: usize,
    from: ScreenOrientation,
    to: ScreenOrientation,
    started_at: Instant,
    duration: Duration,
    source: Vec<Rgb565Pixel>,
    destination: Vec<Rgb565Pixel>,
    output: Vec<Rgb565Pixel>,
    destination_ready: bool,
    active: bool,
    completion: Option<OrientationTransitionCompletion>,
}

impl OrientationTransitionRuntime {
    pub fn new(width: usize, height: usize) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            from: ScreenOrientation::Normal,
            to: ScreenOrientation::Normal,
            started_at: Instant::now(),
            duration: ORIENTATION_QUARTER_TURN_DURATION,
            source: vec![Rgb565Pixel(0); len],
            destination: vec![Rgb565Pixel(0); len],
            output: vec![Rgb565Pixel(0); len],
            destination_ready: false,
            active: false,
            completion: None,
        }
    }

    pub fn start(
        &mut self,
        from: ScreenOrientation,
        to: ScreenOrientation,
        source: &[Rgb565Pixel],
        now: Instant,
        reduce_motion: bool,
    ) -> bool {
        if reduce_motion || from == to || source.len() != self.source.len() {
            self.active = false;
            self.completion = Some(OrientationTransitionCompletion { from, to });
            return false;
        }
        self.from = from;
        self.to = to;
        self.started_at = now;
        self.duration = if from.is_portrait() && to.is_portrait() {
            ORIENTATION_OPPOSITE_TURN_DURATION
        } else {
            ORIENTATION_QUARTER_TURN_DURATION
        };
        self.source.copy_from_slice(source);
        self.destination.fill(Rgb565Pixel(0));
        self.output.copy_from_slice(source);
        self.destination_ready = false;
        self.active = true;
        self.completion = None;
        true
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn capture_destination(&mut self, pixels: &[Rgb565Pixel]) -> bool {
        if !self.active || pixels.len() != self.destination.len() {
            return false;
        }
        if self.destination_ready {
            return true;
        }
        self.destination.copy_from_slice(pixels);
        self.destination_ready = true;
        true
    }

    pub fn destination_ready(&self) -> bool {
        self.destination_ready
    }

    pub fn render(&mut self, now: Instant) -> Option<(&[Rgb565Pixel], bool)> {
        if !self.active {
            return None;
        }
        if !self.destination_ready {
            return Some((&self.source, false));
        }
        let progress = (now.saturating_duration_since(self.started_at).as_secs_f32()
            / self.duration.as_secs_f32())
        .clamp(0.0, 1.0);
        render_rotated_source(
            &self.source,
            &mut self.output,
            self.width,
            self.height,
            transition_quarter_turns(self.from, self.to) as f32 * progress,
        );
        if progress >= DESTINATION_CROSSFADE_START {
            let alpha = (((progress - DESTINATION_CROSSFADE_START)
                / (1.0 - DESTINATION_CROSSFADE_START))
                * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            for (pixel, destination) in self.output.iter_mut().zip(&self.destination) {
                *pixel = blend_565(*pixel, *destination, alpha);
            }
        }
        let done = progress >= 1.0;
        if done {
            self.output.copy_from_slice(&self.destination);
            self.active = false;
            self.completion = Some(OrientationTransitionCompletion {
                from: self.from,
                to: self.to,
            });
        }
        Some((&self.output, done))
    }

    pub fn take_completion(&mut self) -> Option<OrientationTransitionCompletion> {
        self.completion.take()
    }
}

fn orientation_turns(orientation: ScreenOrientation) -> i8 {
    match orientation {
        ScreenOrientation::Normal => 0,
        ScreenOrientation::MonitorClockwise => -1,
        ScreenOrientation::MonitorCounterclockwise => 1,
    }
}

fn transition_quarter_turns(from: ScreenOrientation, to: ScreenOrientation) -> i8 {
    orientation_turns(to) - orientation_turns(from)
}

fn render_rotated_source(
    source: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    quarter_turns: f32,
) {
    let angle = quarter_turns * std::f32::consts::FRAC_PI_2;
    let (sin, cos) = angle.sin_cos();
    let rotated_width = cos.abs() * width as f32 + sin.abs() * height as f32;
    let rotated_height = sin.abs() * width as f32 + cos.abs() * height as f32;
    let scale =
        (width as f32 / rotated_width.max(1.0)).min(height as f32 / rotated_height.max(1.0));
    let source_cx = (width as f32 - 1.0) * 0.5;
    let source_cy = (height as f32 - 1.0) * 0.5;
    output.fill(Rgb565Pixel(0));
    for y in 0..height {
        let dy = (y as f32 - source_cy) / scale;
        let row = y * width;
        for x in 0..width {
            let dx = (x as f32 - source_cx) / scale;
            let source_x = cos * dx + sin * dy + source_cx;
            let source_y = -sin * dx + cos * dy + source_cy;
            if source_x >= 0.0
                && source_y >= 0.0
                && source_x < width as f32
                && source_y < height as f32
            {
                output[row + x] = source[(source_y.round() as usize).min(height - 1) * width
                    + (source_x.round() as usize).min(width - 1)];
            }
        }
    }
}

fn blend_565(source: Rgb565Pixel, destination: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
    if alpha == 0 {
        return source;
    }
    if alpha == u8::MAX {
        return destination;
    }
    let alpha = u32::from(alpha);
    let inverse = 255 - alpha;
    let source = u32::from(source.0);
    let destination = u32::from(destination.0);
    let r = (((source >> 11) & 0x1f) * inverse + ((destination >> 11) & 0x1f) * alpha) / 255;
    let g = (((source >> 5) & 0x3f) * inverse + ((destination >> 5) & 0x3f) * alpha) / 255;
    let b = ((source & 0x1f) * inverse + (destination & 0x1f) * alpha) / 255;
    Rgb565Pixel(((r << 11) | (g << 5) | b) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_portrait_directions_use_the_longer_transition() {
        let start = Instant::now();
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(runtime.start(
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
            &[Rgb565Pixel(1); 12],
            start,
            false,
        ));
        assert_eq!(runtime.duration, ORIENTATION_OPPOSITE_TURN_DURATION);
    }

    #[test]
    fn reduce_motion_completes_without_rendering_transition_frames() {
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(!runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &[Rgb565Pixel(1); 12],
            Instant::now(),
            true,
        ));
        assert!(!runtime.is_active());
        assert_eq!(
            runtime.take_completion(),
            Some(OrientationTransitionCompletion {
                from: ScreenOrientation::Normal,
                to: ScreenOrientation::MonitorClockwise,
            })
        );
    }

    #[test]
    fn completed_frame_is_exact_destination() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        );
        assert!(runtime.capture_destination(&destination));
        let (frame, done) = runtime
            .render(start + ORIENTATION_QUARTER_TURN_DURATION)
            .expect("transition frame");
        assert!(done);
        assert_eq!(frame, destination);
    }
}
