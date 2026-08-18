// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::io;

const CRT_TRIAL_SECS: u64 = 30;
const CRT_PROBE_SECS: u64 = 20;
const CRT_LATCH_SETTLE_TIMEOUT: Duration = Duration::from_millis(100);
const CRT_PROBE_SLOW_PERIOD: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CrtProbePattern {
    FixedA,
    FixedB,
    IdenticalFlip,
    SlowAb,
    FullAb,
    FullAbHold2,
    FullAbHold3,
    FullAbHold4,
    Motion,
    MotionHold2,
    MotionHold3,
    MotionSlow,
    MotionColor,
    PreloadedRulerSlow,
    PreloadedBarsSlow,
}

impl CrtProbePattern {
    fn from_env() -> Option<Self> {
        Self::parse(&std::env::var("MISTER_CRT_PROBE_PATTERN").ok()?)
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "fixed-a" => Some(Self::FixedA),
            "fixed-b" => Some(Self::FixedB),
            "identical-flip" => Some(Self::IdenticalFlip),
            "slow-ab" => Some(Self::SlowAb),
            "full-ab" => Some(Self::FullAb),
            "full-ab-hold2" => Some(Self::FullAbHold2),
            "full-ab-hold3" => Some(Self::FullAbHold3),
            "full-ab-hold4" => Some(Self::FullAbHold4),
            "motion" => Some(Self::Motion),
            "motion-hold2" => Some(Self::MotionHold2),
            "motion-hold3" => Some(Self::MotionHold3),
            "motion-slow" => Some(Self::MotionSlow),
            "motion-color" => Some(Self::MotionColor),
            "preloaded-ruler-slow" => Some(Self::PreloadedRulerSlow),
            "preloaded-bars-slow" => Some(Self::PreloadedBarsSlow),
            _ => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::FixedA => "fixed-a",
            Self::FixedB => "fixed-b",
            Self::IdenticalFlip => "identical-flip",
            Self::SlowAb => "slow-ab",
            Self::FullAb => "full-ab",
            Self::FullAbHold2 => "full-ab-hold2",
            Self::FullAbHold3 => "full-ab-hold3",
            Self::FullAbHold4 => "full-ab-hold4",
            Self::Motion => "motion",
            Self::MotionHold2 => "motion-hold2",
            Self::MotionHold3 => "motion-hold3",
            Self::MotionSlow => "motion-slow",
            Self::MotionColor => "motion-color",
            Self::PreloadedRulerSlow => "preloaded-ruler-slow",
            Self::PreloadedBarsSlow => "preloaded-bars-slow",
        }
    }

    const fn flips_continuously(self) -> bool {
        self.continuous_hold_rasters().is_some()
    }

    const fn continuous_hold_rasters(self) -> Option<u64> {
        match self {
            Self::IdenticalFlip | Self::FullAb | Self::Motion | Self::MotionColor => Some(1),
            Self::FullAbHold2 => Some(2),
            Self::FullAbHold3 => Some(3),
            Self::FullAbHold4 => Some(4),
            Self::MotionHold2 => Some(2),
            Self::MotionHold3 => Some(3),
            Self::MotionSlow => Some(50),
            Self::FixedA
            | Self::FixedB
            | Self::SlowAb
            | Self::PreloadedRulerSlow
            | Self::PreloadedBarsSlow => None,
        }
    }

    const fn is_motion(self) -> bool {
        matches!(
            self,
            Self::Motion
                | Self::MotionHold2
                | Self::MotionHold3
                | Self::MotionSlow
                | Self::MotionColor
        )
    }
}

#[derive(Debug, Default)]
struct CrtProbeTelemetry {
    writes: u64,
    posts: u64,
    unsafe_active_writes: u64,
    pending_writes: u64,
    max_settle_us: u64,
    max_copy_us: u64,
    max_post_us: u64,
    cadence: CrtTrialCadence,
    last_slot: u8,
    last_sequence: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CrtTrialCounters {
    flips: u16,
    posts: u16,
    drops: u16,
}

impl CrtTrialCounters {
    fn from_status(status: crate::fpga::LatchedFbufStatus) -> Self {
        Self {
            flips: status.flip_count,
            posts: status.post_count,
            drops: status.drop_count,
        }
    }

    fn delta(self, before: Self) -> Self {
        Self {
            flips: self.flips.wrapping_sub(before.flips),
            posts: self.posts.wrapping_sub(before.posts),
            drops: self.drops.wrapping_sub(before.drops),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CrtTrialCadence {
    last_completed_at: Option<Instant>,
    max_interval_us: u64,
    missed_intervals: u64,
}

impl CrtTrialCadence {
    fn record(&mut self, completed_at: Instant, nominal_period_us: u64) {
        if let Some(previous) = self.last_completed_at {
            let interval_us = completed_at
                .saturating_duration_since(previous)
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX);
            self.max_interval_us = self.max_interval_us.max(interval_us);
            if interval_us > nominal_period_us.saturating_mul(3) / 2 {
                self.missed_intervals += 1;
            }
        }
        self.last_completed_at = Some(completed_at);
    }
}

pub(super) fn run_crt_trial_loop(
    secs: u64,
    ui: &UiDisplay,
    hardware: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
) {
    let mode = ui.output_route();
    if secs != CRT_TRIAL_SECS || !mode.is_crt() {
        crate::ui_errln!(
            "crt_trial_status_v5 schema=5 ok=0 mode={} reason=invalid-contract requested_secs={} geometry={}x{}",
            mode.label(),
            secs,
            ui.render_w(),
            ui.render_h()
        );
        return;
    }

    let (caps_hi, caps_lo, capabilities) = match hardware.read_magik_latched_fbuf_capabilities() {
        Ok(capabilities) => capabilities,
        Err(error) => {
            crate::ui_errln!(
                "crt_trial_status_v5 schema=5 ok=0 mode={} reason=latch-capabilities detail={}",
                mode.label(),
                safe_field(&error.to_string())
            );
            return;
        }
    };
    let caps_supported = caps_hi == crate::fpga::MAGIK_FBUF_CAPS_MAGIC
        || caps_lo == crate::fpga::MAGIK_FBUF_CAPS_MAGIC;
    if !caps_supported || !capabilities.production_ready() {
        crate::ui_errln!(
            "crt_trial_status_v5 schema=5 ok=0 mode={} reason=latch-capabilities protocol={} flags=0x{:04x}",
            mode.label(),
            capabilities.protocol_version,
            capabilities.flags
        );
        return;
    }

    let initial_status = match wait_for_crt_latch_settle(hardware) {
        Ok(status) if status.supported() => status,
        Ok(status) => {
            crate::ui_errln!(
                "crt_trial_status_v5 schema=5 ok=0 mode={} duration_ms=0 frames=0 flips=0 posts=0 drops=0 reason=latch-status-unsupported ack_high=0x{:04x} ack_low=0x{:04x}",
                mode.label(),
                status.magic_hi,
                status.magic_lo
            );
            return;
        }
        Err(error) => {
            crate::ui_errln!(
                "crt_trial_status_v5 schema=5 ok=0 mode={} reason=latch-status-read detail={}",
                mode.label(),
                safe_field(&error.to_string())
            );
            return;
        }
    };
    let before = CrtTrialCounters::from_status(initial_status);

    let width = ui.render_w();
    let height = ui.render_h();
    let content_bounds = crt_trial_content_bounds_from_env(width);
    let started = Instant::now();
    let mut frames = 0u64;
    let mut frame = vec![Rgb565Pixel(0); width * height];
    let full_damage = DirtyRectList::from_one(DirtyRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    });
    let mut presenter = match FpgaVblankLatchHiddenPresenter::open(ui) {
        Ok(presenter) => presenter,
        Err(failure) => {
            crate::ui_errln!(
                "crt_trial_status_v5 schema=5 ok=0 mode={} reason=presenter-open stage={} detail={}",
                mode.label(),
                failure.stage.code(),
                safe_field(&failure.detail)
            );
            return;
        }
    };
    let mut failure = None;
    let nominal_period_us = mode.nominal_period_us().unwrap_or(20_000);
    let mut cadence = CrtTrialCadence::default();
    let mut settled_status = initial_status;
    let mut last_buffer = None;
    let mut last_sequence = 0;
    let mut unsafe_active_writes = 0u64;
    let mut pending_writes = 0u64;
    let mut alternation_misses = 0u64;
    let mut max_settle_us = 0u64;
    let mut max_render_us = 0u64;
    let mut max_copy_us = 0u128;
    let mut max_status_us = 0u64;
    let mut post_status_metrics = PostStatusObservationMetrics::default();
    while started.elapsed() < Duration::from_secs(CRT_TRIAL_SECS) {
        if frames > 0 {
            let settle_started = Instant::now();
            match wait_for_crt_latch_settle(hardware) {
                Ok(status) => settled_status = status,
                Err(error) => {
                    failure = Some(format!("latch-settle-{}", safe_field(&error.to_string())));
                    break;
                }
            }
            max_settle_us = max_settle_us.max(
                settle_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
        }
        let render_started = Instant::now();
        render_crt_trial_frame(&mut frame, width, height, frames, content_bounds);
        max_render_us = max_render_us.max(
            render_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        let plan = LauncherFramePlan::new(full_damage, None, None, None, None);
        let stats = match presenter.present_cached_full_frame(
            CachedFrameView::new(&frame, width, height),
            plan,
            hardware,
            display_session,
            false,
            |_hidden, _plan, _preview, _arcade| Ok(()),
        ) {
            Ok(stats) => stats,
            Err(error) => {
                failure = Some(format!("{}-{}", error.stage.code(), error.reason_code()));
                break;
            }
        };
        max_copy_us = max_copy_us.max(stats.copy_us);
        max_status_us = max_status_us.max(stats.status_us);
        post_status_metrics.record(stats.post_status_reads, stats.post_status_wire_attempts);
        let selected_base = presenter.buffer_base_addr(stats.buffer_index);
        if settled_status.active_enabled() && settled_status.active_base == selected_base {
            unsafe_active_writes += 1;
        }
        if settled_status.pending() {
            pending_writes += 1;
        }
        if last_buffer == Some(stats.buffer_index) {
            alternation_misses += 1;
        }
        last_buffer = Some(stats.buffer_index);
        last_sequence = stats.posted_sequence;
        cadence.record(Instant::now(), nominal_period_us);
        frames += 1;
    }

    let (counters, final_status, mut failure) =
        finish_crt_trial(before, frames, failure, wait_for_crt_latch_settle(hardware));
    if failure.is_none() && unsafe_active_writes > 0 {
        failure = Some("active-buffer-write".to_string());
    }
    if failure.is_none() && pending_writes > 0 {
        failure = Some("pending-buffer-write".to_string());
    }
    if failure.is_none() && alternation_misses > 0 {
        failure = Some("buffer-alternation-miss".to_string());
    }
    let final_pending = final_status.is_some_and(crate::fpga::LatchedFbufStatus::pending);
    let final_active_matches = final_status.is_some_and(|status| {
        last_buffer.is_some_and(|buffer| {
            status.active_base == presenter.buffer_base_addr(buffer)
                && status.active_sequence == last_sequence
        })
    });
    if failure.is_none() && (!final_active_matches || final_pending) {
        failure = Some("final-active-route-mismatch".to_string());
    }
    let reason = failure.as_deref().unwrap_or(if counters.flips == 0 {
        "no-latch-flips"
    } else {
        "none"
    });
    crate::ui_logln!(
        "crt_trial_status_v5 schema=5 ok={} mode={} duration_ms={} frames={} flips={} posts={} drops={} final_pending={} final_active_matches={} unsafe_active_writes={} pending_writes={} alternation_misses={} cadence_misses={} max_interval_us={} max_settle_us={} max_render_us={} max_copy_us={} max_status_us={} post_status_retry_frames={} max_post_status_reads={} post_status_transport_retry_frames={} max_post_status_wire_attempts={} last_buffer={} last_sequence={} reason={}",
        u8::from(failure.is_none() && frames > 0 && counters.flips > 0),
        mode.label(),
        started.elapsed().as_millis(),
        frames,
        counters.flips,
        counters.posts,
        counters.drops,
        u8::from(final_pending),
        u8::from(final_active_matches),
        unsafe_active_writes,
        pending_writes,
        alternation_misses,
        cadence.missed_intervals,
        cadence.max_interval_us,
        max_settle_us,
        max_render_us,
        max_copy_us,
        max_status_us,
        post_status_metrics.logical_retry_frames,
        post_status_metrics.max_logical_reads,
        post_status_metrics.transport_retry_frames,
        post_status_metrics.max_wire_attempts,
        last_buffer.unwrap_or(0),
        last_sequence,
        reason
    );
}

#[derive(Default)]
struct PostStatusObservationMetrics {
    logical_retry_frames: u64,
    max_logical_reads: u8,
    transport_retry_frames: u64,
    max_wire_attempts: u8,
}

impl PostStatusObservationMetrics {
    fn record(&mut self, logical_reads: u8, wire_attempts: u8) {
        self.logical_retry_frames += u64::from(logical_reads > 1);
        self.max_logical_reads = self.max_logical_reads.max(logical_reads);
        self.transport_retry_frames += u64::from(wire_attempts > logical_reads);
        self.max_wire_attempts = self.max_wire_attempts.max(wire_attempts);
    }
}

pub(super) fn run_crt_probe_loop(
    secs: u64,
    ui: &UiDisplay,
    hardware: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
) {
    let mode = ui.output_route();
    let Some(pattern) = CrtProbePattern::from_env() else {
        crate::ui_errln!(
            "crt_probe_status_v1 schema=1 ok=0 mode={} reason=invalid-pattern",
            mode.label()
        );
        return;
    };
    if secs != CRT_PROBE_SECS || !mode.is_crt() {
        crate::ui_errln!(
            "crt_probe_status_v1 schema=1 ok=0 pattern={} mode={} reason=invalid-contract requested_secs={} geometry={}x{}",
            pattern.label(),
            mode.label(),
            secs,
            ui.fb_w(),
            ui.fb_h()
        );
        return;
    }

    let (caps_hi, caps_lo, capabilities) = match hardware.read_magik_latched_fbuf_capabilities() {
        Ok(capabilities) => capabilities,
        Err(error) => {
            crate::ui_errln!(
                "crt_probe_status_v1 schema=1 ok=0 pattern={} mode={} reason=latch-capabilities detail={}",
                pattern.label(),
                mode.label(),
                safe_field(&error.to_string())
            );
            return;
        }
    };
    let caps_supported = caps_hi == crate::fpga::MAGIK_FBUF_CAPS_MAGIC
        || caps_lo == crate::fpga::MAGIK_FBUF_CAPS_MAGIC;
    if !caps_supported || !capabilities.production_ready() {
        crate::ui_errln!(
            "crt_probe_status_v1 schema=1 ok=0 pattern={} mode={} reason=latch-capabilities protocol={} flags=0x{:04x}",
            pattern.label(),
            mode.label(),
            capabilities.protocol_version,
            capabilities.flags
        );
        return;
    }

    let initial_status = match wait_for_crt_latch_settle(hardware) {
        Ok(status) if status.supported() => status,
        Ok(_) => {
            crate::ui_errln!(
                "crt_probe_status_v1 schema=1 ok=0 pattern={} mode={} reason=latch-status-unsupported",
                pattern.label(),
                mode.label()
            );
            return;
        }
        Err(error) => {
            crate::ui_errln!(
                "crt_probe_status_v1 schema=1 ok=0 pattern={} mode={} reason=latch-status-read detail={}",
                pattern.label(),
                mode.label(),
                safe_field(&error.to_string())
            );
            return;
        }
    };

    let width = ui.fb_w();
    let height = ui.fb_h();
    let mut buffers = match PluginLatchFrameBuffers::open(width, height) {
        Ok(buffers) => buffers,
        Err(failure) => {
            crate::ui_errln!(
                "crt_probe_status_v1 schema=1 ok=0 pattern={} mode={} reason=buffer-open stage={} detail={}",
                pattern.label(),
                mode.label(),
                failure.stage.code(),
                safe_field(&failure.detail)
            );
            return;
        }
    };
    let base_a = buffers.base_addr(1);
    let base_b = buffers.base_addr(2);
    let route = LauncherFramebufferRoute::for_scan(ui.scan_w(), ui.scan_h(), ui.direct_video());
    let geometry = crate::fpga::LatchedFbufGeometry::new_for_route(width as u16, route, 0);
    let before = CrtTrialCounters::from_status(initial_status);
    let mut sequence = initial_status.active_sequence.wrapping_add(1).max(1);
    let mut telemetry = CrtProbeTelemetry::default();

    let neutral = render_crt_probe_pattern(width, height, 0, 0, None);
    let frame_a = match pattern {
        CrtProbePattern::IdenticalFlip => neutral.clone(),
        CrtProbePattern::PreloadedRulerSlow => {
            render_crt_probe_pattern(width, height, 0, 0, Some(0))
        }
        CrtProbePattern::PreloadedBarsSlow => render_preloaded_bar_pattern(width, height, 1),
        CrtProbePattern::MotionColor => render_colored_motion_pattern(width, height, 0),
        _ => render_crt_probe_pattern(width, height, 1, 0, None),
    };
    let frame_b = match pattern {
        CrtProbePattern::IdenticalFlip => neutral,
        CrtProbePattern::PreloadedRulerSlow => {
            render_crt_probe_pattern(width, height, 0, 0, Some(5))
        }
        CrtProbePattern::PreloadedBarsSlow => render_preloaded_bar_pattern(width, height, 2),
        CrtProbePattern::MotionColor => render_colored_motion_pattern(width, height, 0),
        _ => render_crt_probe_pattern(width, height, 2, 24, None),
    };

    let preparation = prepare_probe_slots(
        &mut buffers,
        [&frame_a, &frame_b],
        width,
        height,
        hardware,
        display_session,
        geometry,
        &mut sequence,
        &mut telemetry,
    );
    let mut failure = preparation.err();
    let mut active_slot = wait_for_crt_latch_settle(hardware)
        .ok()
        .and_then(|status| probe_active_slot(status, base_a, base_b))
        .unwrap_or(0);

    if failure.is_none() {
        let target = match pattern {
            CrtProbePattern::FixedA => 1,
            CrtProbePattern::FixedB => 2,
            _ => active_slot,
        };
        if target != 0 && target != active_slot {
            match post_probe_slot(
                &buffers,
                target,
                width,
                height,
                hardware,
                display_session,
                geometry,
                &mut sequence,
                &mut telemetry,
                mode.nominal_period_us().unwrap_or(20_000),
            ) {
                Ok(()) => active_slot = target,
                Err(error) => failure = Some(error),
            }
        }
    }

    let observation_started = Instant::now();
    let mut motion_frame = u64::from(pattern == CrtProbePattern::MotionColor);
    let mut raster_index = 0u64;
    let mut next_slow_flip = observation_started + CRT_PROBE_SLOW_PERIOD;
    while failure.is_none() && observation_started.elapsed() < Duration::from_secs(CRT_PROBE_SECS) {
        let should_flip = if pattern.flips_continuously() {
            true
        } else if matches!(
            pattern,
            CrtProbePattern::SlowAb
                | CrtProbePattern::PreloadedRulerSlow
                | CrtProbePattern::PreloadedBarsSlow
        ) && Instant::now() >= next_slow_flip
        {
            next_slow_flip += CRT_PROBE_SLOW_PERIOD;
            true
        } else {
            false
        };
        if !should_flip {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let target = probe_target_slot(
            active_slot,
            pattern.continuous_hold_rasters().unwrap_or(1),
            raster_index,
        );
        raster_index = raster_index.wrapping_add(1);
        if pattern.is_motion() && target != active_slot {
            let frame = if pattern == CrtProbePattern::MotionColor {
                render_colored_motion_pattern(width, height, motion_frame)
            } else {
                render_crt_probe_pattern(width, height, 0, 0, Some(motion_frame))
            };
            if let Err(error) = write_probe_slot(
                &mut buffers,
                target,
                &frame,
                width,
                hardware,
                &mut telemetry,
            ) {
                failure = Some(error);
                break;
            }
            motion_frame = motion_frame.wrapping_add(1);
        }
        match post_probe_slot(
            &buffers,
            target,
            width,
            height,
            hardware,
            display_session,
            geometry,
            &mut sequence,
            &mut telemetry,
            mode.nominal_period_us().unwrap_or(20_000),
        ) {
            Ok(()) => active_slot = target,
            Err(error) => failure = Some(error),
        }
    }

    let final_status = wait_for_crt_latch_settle(hardware);
    let (counters, final_status, mut failure) =
        finish_crt_probe(before, telemetry.posts, failure, final_status);
    let final_pending = final_status.is_some_and(crate::fpga::LatchedFbufStatus::pending);
    let final_active_matches = final_status.is_some_and(|status| {
        status.active_base == buffers.base_addr(telemetry.last_slot)
            && status.active_sequence == telemetry.last_sequence
    });
    if failure.is_none() && (!final_active_matches || final_pending) {
        failure = Some("final-active-route-mismatch".to_string());
    }
    if failure.is_none() && telemetry.unsafe_active_writes != 0 {
        failure = Some("active-buffer-write".to_string());
    }
    if failure.is_none() && telemetry.pending_writes != 0 {
        failure = Some("pending-buffer-write".to_string());
    }
    let reason = failure.as_deref().unwrap_or("none");
    crate::ui_logln!(
        "crt_probe_status_v1 schema=1 ok={} pattern={} mode={} duration_ms={} slot_a_base=0x{:08x} slot_b_base=0x{:08x} active_slot={} writes={} posts={} flips={} drops={} final_pending={} final_active_matches={} unsafe_active_writes={} pending_writes={} cadence_misses={} max_interval_us={} max_settle_us={} max_copy_us={} max_post_us={} last_sequence={} reason={}",
        u8::from(failure.is_none()),
        pattern.label(),
        mode.label(),
        observation_started.elapsed().as_millis(),
        base_a,
        base_b,
        active_slot,
        telemetry.writes,
        counters.posts,
        counters.flips,
        counters.drops,
        u8::from(final_pending),
        u8::from(final_active_matches),
        telemetry.unsafe_active_writes,
        telemetry.pending_writes,
        telemetry.cadence.missed_intervals,
        telemetry.cadence.max_interval_us,
        telemetry.max_settle_us,
        telemetry.max_copy_us,
        telemetry.max_post_us,
        telemetry.last_sequence,
        reason
    );
}

fn probe_target_slot(active_slot: u8, hold_rasters: u64, raster_index: u64) -> u8 {
    if (raster_index + 1).is_multiple_of(hold_rasters) {
        if active_slot == 1 { 2 } else { 1 }
    } else {
        active_slot
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_probe_slots(
    buffers: &mut PluginLatchFrameBuffers,
    frames: [&[Rgb565Pixel]; 2],
    width: usize,
    height: usize,
    hardware: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
    geometry: crate::fpga::LatchedFbufGeometry,
    sequence: &mut u16,
    telemetry: &mut CrtProbeTelemetry,
) -> Result<(), String> {
    let status = wait_for_crt_latch_settle(hardware)
        .map_err(|error| format!("prepare-status-{}", safe_field(&error.to_string())))?;
    let active = probe_active_slot(status, buffers.base_addr(1), buffers.base_addr(2));
    let first = if active == Some(1) { 2 } else { 1 };
    write_probe_slot(
        buffers,
        first,
        frames[(first - 1) as usize],
        width,
        hardware,
        telemetry,
    )?;
    post_probe_slot(
        buffers,
        first,
        width,
        height,
        hardware,
        display_session,
        geometry,
        sequence,
        telemetry,
        20_000,
    )?;
    let second = if first == 1 { 2 } else { 1 };
    write_probe_slot(
        buffers,
        second,
        frames[(second - 1) as usize],
        width,
        hardware,
        telemetry,
    )
}

fn write_probe_slot(
    buffers: &mut PluginLatchFrameBuffers,
    slot: u8,
    frame: &[Rgb565Pixel],
    width: usize,
    hardware: &mut Fpga,
    telemetry: &mut CrtProbeTelemetry,
) -> Result<(), String> {
    let status = wait_for_crt_latch_settle(hardware)
        .map_err(|error| format!("write-status-{}", safe_field(&error.to_string())))?;
    if status.pending() {
        telemetry.pending_writes += 1;
        return Err("pending-buffer-write".to_string());
    }
    if status.active_enabled() && status.active_base == buffers.base_addr(slot) {
        telemetry.unsafe_active_writes += 1;
        return Err("active-buffer-write".to_string());
    }
    let started = Instant::now();
    let buffer = buffers.buffer_mut(slot);
    buffer
        .copy_full_frame(frame, width)
        .map_err(|error| format!("frame-copy-{}", safe_field(&error.to_string())))?;
    buffer.publish_writes();
    telemetry.max_copy_us = telemetry
        .max_copy_us
        .max(started.elapsed().as_micros().try_into().unwrap_or(u64::MAX));
    telemetry.writes += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn post_probe_slot(
    buffers: &PluginLatchFrameBuffers,
    slot: u8,
    width: usize,
    height: usize,
    hardware: &mut Fpga,
    _display_session: &mut LauncherDisplaySession,
    geometry: crate::fpga::LatchedFbufGeometry,
    sequence: &mut u16,
    telemetry: &mut CrtProbeTelemetry,
    nominal_period_us: u64,
) -> Result<(), String> {
    let before = wait_for_crt_latch_settle(hardware)
        .map_err(|error| format!("post-before-{}", safe_field(&error.to_string())))?;
    if before.pending() {
        return Err("post-while-pending".to_string());
    }
    let posted_sequence = *sequence;
    *sequence = (*sequence).wrapping_add(1).max(1);
    let started = Instant::now();
    let post = hardware
        .post_latched_rgb565(
            posted_sequence,
            buffers.base_addr(slot),
            width as u16,
            height as u16,
            geometry,
        )
        .map_err(|error| format!("latch-post-{}", safe_field(&error.to_string())))?;
    if post.ack_high != crate::fpga::MAGIK_FBUF_LATCH_MAGIC
        && post.ack_low != crate::fpga::MAGIK_FBUF_LATCH_MAGIC
    {
        return Err("latch-post-unsupported".to_string());
    }
    let settle_started = Instant::now();
    let after = wait_for_crt_post_settle(hardware, posted_sequence)
        .map_err(|error| format!("post-settle-{}", safe_field(&error.to_string())))?;
    telemetry.max_settle_us = telemetry.max_settle_us.max(
        settle_started
            .elapsed()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX),
    );
    telemetry.max_post_us = telemetry
        .max_post_us
        .max(started.elapsed().as_micros().try_into().unwrap_or(u64::MAX));
    if after.pending()
        || after.active_base != buffers.base_addr(slot)
        || after.active_sequence != posted_sequence
        || after.active_width != width as u16
        || after.active_height != height as u16
        || after.active_stride != geometry.stride_bytes
    {
        return Err("post-verification-mismatch".to_string());
    }
    telemetry.posts += 1;
    telemetry.last_slot = slot;
    telemetry.last_sequence = posted_sequence;
    telemetry.cadence.record(Instant::now(), nominal_period_us);
    Ok(())
}

fn probe_active_slot(
    status: crate::fpga::LatchedFbufStatus,
    base_a: u32,
    base_b: u32,
) -> Option<u8> {
    match status.active_base {
        base if status.active_enabled() && base == base_a => Some(1),
        base if status.active_enabled() && base == base_b => Some(2),
        _ => None,
    }
}

fn finish_crt_probe(
    before: CrtTrialCounters,
    expected_posts: u64,
    mut failure: Option<String>,
    final_status: io::Result<crate::fpga::LatchedFbufStatus>,
) -> (
    CrtTrialCounters,
    Option<crate::fpga::LatchedFbufStatus>,
    Option<String>,
) {
    let (counters, final_status, finish_failure) =
        finish_crt_trial(before, expected_posts, failure.take(), final_status);
    failure = finish_failure;
    (counters, final_status, failure)
}

fn render_crt_probe_pattern(
    width: usize,
    height: usize,
    identity: u8,
    offset_x: usize,
    moving_frame: Option<u64>,
) -> Vec<Rgb565Pixel> {
    let mut frame = vec![Rgb565Pixel(0x0841); width * height];
    for y in 0..height {
        for x in 0..width {
            let shifted_x = (x + width - offset_x % width) % width;
            let value = if y % 16 == 0 {
                0xffff
            } else if shifted_x % 32 < 2 {
                0x8410
            } else if shifted_x % 8 == 0 {
                0x4208
            } else {
                0x0841
            };
            frame[y * width + x] = Rgb565Pixel(value);
        }
    }
    let identity_color = match identity {
        1 => 0x07ff,
        2 => 0xf81f,
        _ => 0xffe0,
    };
    render_probe_identity_bands(&mut frame, width, height, identity_color);
    if let Some(frame_number) = moving_frame {
        let marker_x = ((frame_number.wrapping_mul(5)) % width as u64) as usize;
        for y in 0..height {
            for dx in 0..3 {
                frame[y * width + (marker_x + dx) % width] = Rgb565Pixel(0xffff);
            }
        }
    }
    frame
}

fn render_probe_identity_bands(frame: &mut [Rgb565Pixel], width: usize, height: usize, color: u16) {
    let band_height = 12usize.min(height / 2);
    for y in 0..band_height {
        for x in 0..width {
            frame[y * width + x] = Rgb565Pixel(color);
            frame[(height - 1 - y) * width + x] = Rgb565Pixel(color);
        }
    }
}

fn render_preloaded_bar_pattern(width: usize, height: usize, identity: u8) -> Vec<Rgb565Pixel> {
    let mut frame = vec![Rgb565Pixel(0x0000); width * height];
    let (center_x, color) = if identity == 1 {
        (width / 4, 0x07ff)
    } else {
        (width * 3 / 4, 0xf81f)
    };
    let half_width = 12usize.min(width / 8);
    for y in 0..height {
        for x in center_x.saturating_sub(half_width)..(center_x + half_width).min(width) {
            frame[y * width + x] = Rgb565Pixel(color);
        }
    }
    render_probe_identity_bands(&mut frame, width, height, color);
    frame
}

fn render_colored_motion_pattern(
    width: usize,
    height: usize,
    frame_number: u64,
) -> Vec<Rgb565Pixel> {
    const COLORS: [u16; 6] = [0xf800, 0x07ff, 0xffe0, 0x001f, 0xf81f, 0x07e0];
    const STEP: usize = 12;
    const BAR_WIDTH: usize = 24;

    let mut frame = vec![Rgb565Pixel(0x0000); width * height];
    let color = COLORS[frame_number as usize % COLORS.len()];
    let usable_width = width.saturating_sub(BAR_WIDTH).max(1);
    let left = (frame_number as usize).wrapping_mul(STEP) % usable_width;
    let right = (left + BAR_WIDTH).min(width);
    for y in 0..height {
        for x in left..right {
            frame[y * width + x] = Rgb565Pixel(color);
        }
    }
    render_probe_identity_bands(&mut frame, width, height, color);
    frame
}

fn wait_for_crt_latch_settle(hardware: &mut Fpga) -> io::Result<crate::fpga::LatchedFbufStatus> {
    wait_for_crt_latch_settle_with(
        || hardware.read_magik_latched_fbuf_status(),
        CRT_LATCH_SETTLE_TIMEOUT,
        std::thread::sleep,
    )
}

fn wait_for_crt_post_settle(
    hardware: &mut Fpga,
    posted_sequence: u16,
) -> io::Result<crate::fpga::LatchedFbufStatus> {
    wait_for_crt_post_settle_with(
        || hardware.read_magik_latched_fbuf_status(),
        posted_sequence,
        CRT_LATCH_SETTLE_TIMEOUT,
        std::thread::sleep,
    )
}

fn wait_for_crt_post_settle_with(
    mut read_status: impl FnMut() -> io::Result<crate::fpga::LatchedFbufStatus>,
    posted_sequence: u16,
    timeout: Duration,
    mut sleep: impl FnMut(Duration),
) -> io::Result<crate::fpga::LatchedFbufStatus> {
    let started = Instant::now();
    let mut post_observed = false;
    loop {
        let status = read_status()?;
        if !status.supported() {
            return Ok(status);
        }
        if !status.pending() && status.active_sequence == posted_sequence {
            return Ok(status);
        }
        post_observed |= status.pending() && status.pending_sequence == posted_sequence;
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                if post_observed {
                    "posted-latch-did-not-settle"
                } else {
                    "posted-latch-not-observed"
                },
            ));
        }
        sleep(Duration::from_millis(1));
    }
}

fn wait_for_crt_latch_settle_with(
    mut read_status: impl FnMut() -> io::Result<crate::fpga::LatchedFbufStatus>,
    timeout: Duration,
    mut sleep: impl FnMut(Duration),
) -> io::Result<crate::fpga::LatchedFbufStatus> {
    let started = Instant::now();
    loop {
        let status = read_status()?;
        if !status.supported() || !status.pending() {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "pending-latch-did-not-settle",
            ));
        }
        sleep(Duration::from_millis(1));
    }
}

fn finish_crt_trial(
    before: CrtTrialCounters,
    frames: u64,
    mut failure: Option<String>,
    final_status: io::Result<crate::fpga::LatchedFbufStatus>,
) -> (
    CrtTrialCounters,
    Option<crate::fpga::LatchedFbufStatus>,
    Option<String>,
) {
    let final_status = match final_status {
        Ok(status) if status.supported() => Some(status),
        Ok(_) => {
            failure.get_or_insert_with(|| "final-latch-status-unsupported".to_string());
            None
        }
        Err(error) => {
            failure.get_or_insert_with(|| {
                format!("final-latch-settle-{}", safe_field(&error.to_string()))
            });
            None
        }
    };
    let counters = final_status
        .map(CrtTrialCounters::from_status)
        .unwrap_or(before)
        .delta(before);
    if failure.is_none()
        && (u64::from(counters.flips) != frames || u64::from(counters.posts) != frames)
    {
        failure = Some("incomplete-latch-flips".to_string());
    }
    if failure.is_none() && counters.drops != 0 {
        failure = Some("unexpected-latch-drops".to_string());
    }
    (counters, final_status, failure)
}

fn crt_trial_content_bounds_from_env(width: usize) -> Option<(usize, usize)> {
    let value = std::env::var("MISTER_CRT_TRIAL_CONTENT_BOUNDS").ok()?;
    let (left, right) = value.split_once(',')?;
    let left = left.parse::<usize>().ok()?;
    let right = right.parse::<usize>().ok()?;
    (left <= right && right < width).then_some((left, right))
}

fn render_crt_trial_frame(
    dst: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    frame: u64,
    content_bounds: Option<(usize, usize)>,
) {
    debug_assert_eq!(dst.len(), width * height);
    const BORDER: usize = 2;
    const BARS: [u16; 8] = [
        0xffff, 0xffe0, 0x07ff, 0x07e0, 0xf81f, 0xf800, 0x001f, 0x0000,
    ];
    let (content_left, content_right) = content_bounds.unwrap_or((0, width - 1));
    let content_width = content_right - content_left + 1;
    for y in 0..height {
        for x in 0..width {
            let value = if x < content_left || x > content_right {
                0x0000
            } else if y < height / 2 {
                let content_x = x - content_left;
                BARS[(content_x * BARS.len() / content_width).min(BARS.len() - 1)]
            } else if (x - content_left) % 16 == 0 || y % 16 == 0 {
                0x8410
            } else {
                0x1082
            };
            dst[y * width + x] = Rgb565Pixel(value);
        }
    }
    let marker_x = content_left + ((frame * 5) % content_width as u64) as usize;
    for y in 0..height {
        for dx in 0..3 {
            let x = content_left + (marker_x - content_left + dx) % content_width;
            dst[y * width + x] = Rgb565Pixel(0xffff);
        }
    }
    render_frame_code_band(
        dst,
        width,
        height,
        frame,
        content_left,
        content_right,
        BORDER,
    );
    for y in 0..height {
        for x in content_left..=content_right {
            if x < content_left + BORDER
                || x > content_right.saturating_sub(BORDER)
                || y < BORDER
                || y >= height - BORDER
            {
                dst[y * width + x] = Rgb565Pixel(0xffff);
            }
        }
    }
}

/// Draws the same frame identity at both raster edges so a mixed lower field
/// is visible without relying on motion in the trial scene.
fn render_frame_code_band(
    dst: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    frame: u64,
    content_left: usize,
    content_right: usize,
    border: usize,
) {
    const CODE_BITS: usize = 16;
    const BAND_HEIGHT: usize = 8;
    if height < border * 2 + BAND_HEIGHT * 2 {
        return;
    }
    let content_width = content_right - content_left + 1;
    for (y0, y1) in [
        (border, border + BAND_HEIGHT),
        (height - border - BAND_HEIGHT, height - border),
    ] {
        for y in y0..y1 {
            for x in content_left..=content_right {
                let bit = ((x - content_left) * CODE_BITS / content_width).min(CODE_BITS - 1);
                let value = if frame & (1 << bit) != 0 {
                    0xffff
                } else {
                    0x0000
                };
                dst[y * width + x] = Rgb565Pixel(value);
            }
        }
    }
}

fn safe_field(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_status_metrics_distinguish_logical_observation_from_transport_retry() {
        let mut metrics = PostStatusObservationMetrics::default();

        metrics.record(1, 2);
        metrics.record(3, 3);

        assert_eq!(metrics.logical_retry_frames, 1);
        assert_eq!(metrics.max_logical_reads, 3);
        assert_eq!(metrics.transport_retry_frames, 1);
        assert_eq!(metrics.max_wire_attempts, 3);
    }

    fn status(flips: u16, pending: bool) -> crate::fpga::LatchedFbufStatus {
        crate::fpga::LatchedFbufStatus {
            magic_hi: crate::fpga::MAGIK_FBUF_STATUS_MAGIC,
            magic_lo: 0,
            active_sequence: 1,
            pending_sequence: 2,
            flags: 0x0001 | if pending { 0x0004 } else { 0 },
            flip_count: flips,
            post_count: flips,
            drop_count: 0,
            active_base: 0x227e_9000,
            active_width: 640,
            active_height: 480,
            active_stride: 1280,
            reject_count: 0,
            active_route_epoch: 0,
            accepted_sequence: if pending { 2 } else { 1 },
            active_transaction: 1,
            pending_transaction: if pending { 2 } else { 0 },
            accepted_transaction: if pending { 2 } else { 1 },
        }
    }

    #[test]
    fn pattern_scales_to_each_standard_crt_height() {
        for height in [240, 288, 480, 576] {
            let mut first = vec![Rgb565Pixel(0); 640 * height];
            let mut second = first.clone();
            render_crt_trial_frame(&mut first, 640, height, 0, None);
            render_crt_trial_frame(&mut second, 640, height, 1, None);

            assert_eq!(first[10 * 640 + 100].0, 0xffe0);
            assert_eq!(first[(height / 2) * 640 + 32].0, 0x8410);
            assert_eq!(first[50 * 640].0, 0xffff);
            assert_eq!(second[50 * 640 + 5].0, 0xffff);
            assert_eq!(first[height * 640 - 1].0, 0xffff);
            assert_eq!(first[(height - 1) * 640 + 320].0, 0xffff);
            assert_eq!(first[100 * 640 + 639].0, 0xffff);
            assert_ne!(first, second);
            assert_eq!(
                &second[2 * 640..10 * 640],
                &second[(height - 10) * 640..(height - 2) * 640]
            );
        }
    }

    #[test]
    fn frame_code_changes_and_matches_at_the_top_and_bottom() {
        let mut first = vec![Rgb565Pixel(0); 640 * 288];
        let mut second = first.clone();
        render_crt_trial_frame(&mut first, 640, 288, 1, None);
        render_crt_trial_frame(&mut second, 640, 288, 2, None);

        assert_ne!(&first[2 * 640..10 * 640], &second[2 * 640..10 * 640]);
        assert_eq!(
            &second[2 * 640..10 * 640],
            &second[(288 - 10) * 640..(288 - 2) * 640]
        );
    }

    #[test]
    fn bounded_pattern_compresses_complete_content_inside_black_margins() {
        let mut frame = vec![Rgb565Pixel(0); 640 * 480];

        render_crt_trial_frame(&mut frame, 640, 480, 0, Some((64, 575)));

        assert_eq!(frame[100 * 640 + 63].0, 0x0000);
        assert_eq!(frame[100 * 640 + 64].0, 0xffff);
        assert_eq!(frame[100 * 640 + 575].0, 0xffff);
        assert_eq!(frame[100 * 640 + 576].0, 0x0000);
        assert_eq!(frame[10 * 640 + 128].0, 0xffe0);
        assert_eq!(frame[(480 - 1) * 640 + 320].0, 0xffff);
    }

    #[test]
    fn flip_counter_delta_wraps_without_panicking() {
        let before = CrtTrialCounters {
            flips: u16::MAX,
            posts: u16::MAX,
            drops: u16::MAX,
        };
        let after = CrtTrialCounters {
            flips: 1,
            posts: 1,
            drops: 1,
        };
        assert_eq!(
            after.delta(before),
            CrtTrialCounters {
                flips: 2,
                posts: 2,
                drops: 2,
            }
        );
    }

    #[test]
    fn cadence_flags_only_intervals_over_one_and_a_half_frames() {
        let start = Instant::now();
        let mut cadence = CrtTrialCadence::default();

        cadence.record(start, 20_000);
        cadence.record(start + Duration::from_micros(30_000), 20_000);
        cadence.record(start + Duration::from_micros(60_001), 20_000);

        assert_eq!(cadence.max_interval_us, 30_001);
        assert_eq!(cadence.missed_intervals, 1);
    }

    #[test]
    fn final_latch_wait_accepts_pending_then_settled() {
        let mut statuses = vec![status(10, false), status(9, true)];
        let settled = wait_for_crt_latch_settle_with(
            || Ok(statuses.pop().expect("scripted status")),
            Duration::from_millis(10),
            |_| {},
        )
        .expect("pending latch should settle");

        assert_eq!(settled.flip_count, 10);
        assert!(!settled.pending());
    }

    #[test]
    fn final_latch_wait_times_out_when_pending_never_clears() {
        let error = wait_for_crt_latch_settle_with(|| Ok(status(9, true)), Duration::ZERO, |_| {})
            .expect_err("permanent pending latch must time out");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn post_latch_wait_ignores_stale_active_status_until_post_settles() {
        let mut stale = status(9, false);
        stale.active_sequence = 8;
        let mut pending = status(9, true);
        pending.active_sequence = 8;
        pending.pending_sequence = 9;
        let mut settled_status = status(10, false);
        settled_status.active_sequence = 9;
        let mut statuses = vec![settled_status, pending, stale];

        let settled = wait_for_crt_post_settle_with(
            || Ok(statuses.pop().expect("scripted status")),
            9,
            Duration::from_millis(10),
            |_| {},
        )
        .expect("posted latch should be observed and settle");

        assert_eq!(settled.active_sequence, 9);
        assert!(!settled.pending());
    }

    #[test]
    fn post_latch_wait_rejects_a_post_that_is_never_observed() {
        let mut stale = status(8, false);
        stale.active_sequence = 8;
        let error = wait_for_crt_post_settle_with(|| Ok(stale), 9, Duration::ZERO, |_| {})
            .expect_err("stale active status must not verify a new post");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "posted-latch-not-observed");
    }

    #[test]
    fn final_completion_requires_supported_settled_status_and_matching_flips() {
        let before = CrtTrialCounters {
            flips: 20,
            posts: 20,
            drops: 0,
        };
        let (counters, _, failure) = finish_crt_trial(before, 2, None, Ok(status(22, false)));
        assert_eq!(counters.flips, 2);
        assert_eq!(counters.posts, 2);
        assert_eq!(counters.drops, 0);
        assert_eq!(failure, None);

        let (counters, _, failure) = finish_crt_trial(before, 2, None, Ok(status(21, false)));
        assert_eq!(counters.flips, 1);
        assert_eq!(failure.as_deref(), Some("incomplete-latch-flips"));

        let mut unsupported = status(22, false);
        unsupported.magic_hi = 0;
        let (_, _, failure) = finish_crt_trial(before, 2, None, Ok(unsupported));
        assert_eq!(failure.as_deref(), Some("final-latch-status-unsupported"));

        let (_, _, failure) = finish_crt_trial(
            before,
            2,
            None,
            Err(io::Error::new(io::ErrorKind::TimedOut, "pending")),
        );
        assert_eq!(failure.as_deref(), Some("final-latch-settle-pending"));
    }

    #[test]
    fn probe_pattern_names_are_closed_and_typed() {
        assert_eq!(
            CrtProbePattern::parse("fixed-a"),
            Some(CrtProbePattern::FixedA)
        );
        assert_eq!(
            CrtProbePattern::parse("identical-flip"),
            Some(CrtProbePattern::IdenticalFlip)
        );
        assert_eq!(
            CrtProbePattern::parse("motion"),
            Some(CrtProbePattern::Motion)
        );
        assert_eq!(
            CrtProbePattern::parse("motion-hold2"),
            Some(CrtProbePattern::MotionHold2)
        );
        assert_eq!(
            CrtProbePattern::parse("motion-slow"),
            Some(CrtProbePattern::MotionSlow)
        );
        assert_eq!(
            CrtProbePattern::parse("motion-color"),
            Some(CrtProbePattern::MotionColor)
        );
        assert_eq!(
            CrtProbePattern::parse("preloaded-ruler-slow"),
            Some(CrtProbePattern::PreloadedRulerSlow)
        );
        assert_eq!(
            CrtProbePattern::parse("preloaded-bars-slow"),
            Some(CrtProbePattern::PreloadedBarsSlow)
        );
        assert_eq!(
            CrtProbePattern::parse("full-ab-hold4"),
            Some(CrtProbePattern::FullAbHold4)
        );
        assert_eq!(CrtProbePattern::parse("arbitrary"), None);
    }

    #[test]
    fn rate_sweep_holds_each_ab_slot_for_the_requested_rasters() {
        for hold_rasters in 1..=4 {
            let mut active_slot = 1;
            let observed = (0..8)
                .map(|raster_index| {
                    active_slot = probe_target_slot(active_slot, hold_rasters, raster_index);
                    active_slot
                })
                .collect::<Vec<_>>();
            let expected = (0..8)
                .map(|raster_index| {
                    if ((raster_index + 1) / hold_rasters).is_multiple_of(2) {
                        1
                    } else {
                        2
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(observed, expected, "hold_rasters={hold_rasters}");
        }
    }

    #[test]
    fn probe_identity_matches_at_both_raster_edges() {
        let frame = render_crt_probe_pattern(640, 576, 1, 0, None);

        assert_eq!(&frame[..12 * 640], &frame[(576 - 12) * 640..]);
        assert!(frame[..12 * 640].iter().all(|pixel| pixel.0 == 0x07ff));
    }

    #[test]
    fn ab_probe_has_exact_twenty_four_pixel_ruler_offset() {
        let frame_a = render_crt_probe_pattern(640, 576, 1, 0, None);
        let frame_b = render_crt_probe_pattern(640, 576, 2, 24, None);
        let row = 33;

        assert_eq!(frame_a[row * 640].0, 0x8410);
        assert_eq!(frame_b[row * 640 + 24].0, 0x8410);
        assert_ne!(frame_a[row * 640], frame_b[row * 640]);
    }

    #[test]
    fn motion_probe_moves_five_pixels_per_frame() {
        let first = render_crt_probe_pattern(640, 576, 0, 0, Some(3));
        let second = render_crt_probe_pattern(640, 576, 0, 0, Some(4));
        let row = 100;

        assert_eq!(first[row * 640 + 15].0, 0xffff);
        assert_eq!(second[row * 640 + 20].0, 0xffff);
        assert_ne!(first, second);
    }

    #[test]
    fn colored_motion_probe_encodes_frame_age_in_position_and_color() {
        let first = render_colored_motion_pattern(640, 576, 0);
        let second = render_colored_motion_pattern(640, 576, 1);
        let fourth = render_colored_motion_pattern(640, 576, 3);
        let middle_row = 288;

        assert_eq!(first[middle_row * 640].0, 0xf800);
        assert_eq!(second[middle_row * 640 + 12].0, 0x07ff);
        assert_eq!(fourth[middle_row * 640 + 36].0, 0x001f);
        assert_eq!(second[0].0, 0x07ff);
        assert_eq!(second[575 * 640].0, 0x07ff);
        assert_ne!(first, second);
        assert_ne!(second, fourth);
    }

    #[test]
    fn preloaded_ruler_positions_are_twenty_five_pixels_apart() {
        let first = render_crt_probe_pattern(640, 576, 0, 0, Some(0));
        let second = render_crt_probe_pattern(640, 576, 0, 0, Some(5));
        let row = 100;

        assert_eq!(first[row * 640].0, 0xffff);
        assert_eq!(second[row * 640 + 25].0, 0xffff);
        assert_ne!(first, second);
    }

    #[test]
    fn preloaded_bars_have_unambiguous_positions_and_identities() {
        let left = render_preloaded_bar_pattern(640, 576, 1);
        let right = render_preloaded_bar_pattern(640, 576, 2);
        let middle_row = 288;

        assert_eq!(left[middle_row * 640 + 160].0, 0x07ff);
        assert_eq!(left[middle_row * 640 + 480].0, 0x0000);
        assert_eq!(right[middle_row * 640 + 160].0, 0x0000);
        assert_eq!(right[middle_row * 640 + 480].0, 0xf81f);
        assert_eq!(left[0].0, 0x07ff);
        assert_eq!(right[575 * 640].0, 0xf81f);
    }

    #[test]
    fn motion_rate_sweep_updates_only_when_the_displayed_slot_changes() {
        for (hold_rasters, raster_count, expected_updates) in
            [(1, 8, 8), (2, 8, 4), (3, 8, 2), (50, 100, 2)]
        {
            let mut active_slot = 1;
            let mut updates = 0;
            for raster_index in 0..raster_count {
                let target = probe_target_slot(active_slot, hold_rasters, raster_index);
                if target != active_slot {
                    updates += 1;
                }
                active_slot = target;
            }
            assert_eq!(updates, expected_updates, "hold_rasters={hold_rasters}");
        }
    }
}
