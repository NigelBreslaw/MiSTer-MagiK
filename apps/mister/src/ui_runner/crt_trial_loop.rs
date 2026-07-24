// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::io;

const CRT_TRIAL_SECS: u64 = 30;
const CRT_LATCH_SETTLE_TIMEOUT: Duration = Duration::from_millis(100);

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
            "crt_trial_status_v3 schema=3 ok=0 mode={} reason=invalid-contract requested_secs={} geometry={}x{}",
            mode.label(),
            secs,
            ui.render_w(),
            ui.render_h()
        );
        return;
    }

    let initial_status = match wait_for_crt_latch_settle(hardware) {
        Ok(status) if status.supported() => status,
        Ok(status) => {
            crate::ui_errln!(
                "crt_trial_status_v3 schema=3 ok=0 mode={} duration_ms=0 frames=0 flips=0 posts=0 drops=0 reason=latch-status-unsupported ack_high=0x{:04x} ack_low=0x{:04x}",
                mode.label(),
                status.magic_hi,
                status.magic_lo
            );
            return;
        }
        Err(error) => {
            crate::ui_errln!(
                "crt_trial_status_v3 schema=3 ok=0 mode={} reason=latch-status-read detail={}",
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
                "crt_trial_status_v3 schema=3 ok=0 mode={} reason=presenter-open stage={} detail={}",
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
    let mut post_status_retry_frames = 0u64;
    let mut max_post_status_reads = 0u8;
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
            |_hidden, _plan| Ok(()),
        ) {
            Ok(stats) => stats,
            Err(error) => {
                failure = Some(format!("{}-{}", error.stage.code(), error.reason_code()));
                break;
            }
        };
        max_copy_us = max_copy_us.max(stats.copy_us);
        max_status_us = max_status_us.max(stats.status_us);
        max_post_status_reads = max_post_status_reads.max(stats.post_status_reads);
        post_status_retry_frames += u64::from(stats.post_status_reads > 1);
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
        "crt_trial_status_v3 schema=3 ok={} mode={} duration_ms={} frames={} flips={} posts={} drops={} final_pending={} final_active_matches={} unsafe_active_writes={} pending_writes={} alternation_misses={} cadence_misses={} max_interval_us={} max_settle_us={} max_render_us={} max_copy_us={} max_status_us={} post_status_retry_frames={} max_post_status_reads={} last_buffer={} last_sequence={} reason={}",
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
        post_status_retry_frames,
        max_post_status_reads,
        last_buffer.unwrap_or(0),
        last_sequence,
        reason
    );
}

fn wait_for_crt_latch_settle(hardware: &mut Fpga) -> io::Result<crate::fpga::LatchedFbufStatus> {
    wait_for_crt_latch_settle_with(
        || hardware.read_magik_latched_fbuf_status(),
        CRT_LATCH_SETTLE_TIMEOUT,
        std::thread::sleep,
    )
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
}
