// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

const CRT_TRIAL_SECS: u64 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CrtTrialCounters {
    flips: u16,
}

impl CrtTrialCounters {
    fn from_status(status: crate::fpga::LatchedFbufStatus) -> Self {
        Self {
            flips: status.flip_count,
        }
    }

    fn delta(self, before: Self) -> Self {
        Self {
            flips: self.flips.wrapping_sub(before.flips),
        }
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
            "crt_trial_status_v2 schema=2 ok=0 mode={} reason=invalid-contract requested_secs={} geometry={}x{}",
            mode.label(),
            secs,
            ui.render_w(),
            ui.render_h()
        );
        return;
    }

    let before = match hardware.read_magik_latched_fbuf_status() {
        Ok(status) if status.supported() => CrtTrialCounters::from_status(status),
        Ok(status) => {
            crate::ui_errln!(
                "crt_trial_status_v2 schema=2 ok=0 mode={} duration_ms=0 frames=0 flips=0 reason=latch-status-unsupported ack_high=0x{:04x} ack_low=0x{:04x}",
                mode.label(),
                status.magic_hi,
                status.magic_lo
            );
            return;
        }
        Err(error) => {
            crate::ui_errln!(
                "crt_trial_status_v2 schema=2 ok=0 mode={} reason=latch-status-read detail={}",
                mode.label(),
                safe_field(&error.to_string())
            );
            return;
        }
    };

    let width = ui.render_w();
    let height = ui.render_h();
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
                "crt_trial_status_v2 schema=2 ok=0 mode={} reason=presenter-open stage={} detail={}",
                mode.label(),
                failure.stage.code(),
                safe_field(&failure.detail)
            );
            return;
        }
    };
    let mut failure = None;
    while started.elapsed() < Duration::from_secs(CRT_TRIAL_SECS) {
        render_crt_trial_frame(&mut frame, width, height, frames);
        let plan = LauncherFramePlan::new(full_damage, None, None, None, None);
        if let Err(error) = presenter.present_cached_full_frame(
            CachedFrameView::new(&frame, width, height),
            plan,
            hardware,
            display_session,
            |_hidden, _plan| Ok(()),
        ) {
            failure = Some(format!("{}-{}", error.stage.code(), error.reason_code()));
            break;
        }
        frames += 1;
    }

    let flips = hardware
        .read_magik_latched_fbuf_status()
        .ok()
        .filter(|status| status.supported())
        .map(CrtTrialCounters::from_status)
        .unwrap_or(before)
        .delta(before)
        .flips;
    let reason = failure
        .as_deref()
        .unwrap_or(if flips == 0 { "no-latch-flips" } else { "none" });
    crate::ui_logln!(
        "crt_trial_status_v2 schema=2 ok={} mode={} duration_ms={} frames={} flips={} reason={}",
        u8::from(failure.is_none() && frames > 0 && flips > 0),
        mode.label(),
        started.elapsed().as_millis(),
        frames,
        flips,
        reason
    );
}

fn render_crt_trial_frame(dst: &mut [Rgb565Pixel], width: usize, height: usize, frame: u64) {
    debug_assert_eq!(dst.len(), width * height);
    const BARS: [u16; 8] = [
        0xffff, 0xffe0, 0x07ff, 0x07e0, 0xf81f, 0xf800, 0x001f, 0x0000,
    ];
    for y in 0..height {
        for x in 0..width {
            let value = if y < height / 2 {
                BARS[(x * BARS.len() / width).min(BARS.len() - 1)]
            } else if x % 16 == 0 || y % 16 == 0 {
                0x8410
            } else {
                0x1082
            };
            dst[y * width + x] = Rgb565Pixel(value);
        }
    }
    let marker_x = ((frame * 5) % width as u64) as usize;
    for y in 0..height {
        for dx in 0..3 {
            dst[y * width + (marker_x + dx) % width] = Rgb565Pixel(0xffff);
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
    fn pattern_scales_to_each_standard_crt_height() {
        for height in [240, 288, 480, 576] {
            let mut first = vec![Rgb565Pixel(0); 640 * height];
            let mut second = first.clone();
            render_crt_trial_frame(&mut first, 640, height, 0);
            render_crt_trial_frame(&mut second, 640, height, 1);

            assert_eq!(first[10 * 640 + 100].0, 0xffe0);
            assert_eq!(first[(height / 2) * 640 + 32].0, 0x8410);
            assert_eq!(first[50 * 640].0, 0xffff);
            assert_eq!(second[50 * 640 + 5].0, 0xffff);
            assert_ne!(first, second);
        }
    }

    #[test]
    fn flip_counter_delta_wraps_without_panicking() {
        let before = CrtTrialCounters { flips: u16::MAX };
        let after = CrtTrialCounters { flips: 1 };
        assert_eq!(after.delta(before), CrtTrialCounters { flips: 2 });
    }
}
