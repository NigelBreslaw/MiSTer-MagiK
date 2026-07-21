// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

const CRT_TRIAL_SECS: u64 = 30;
const CRT_WIDTH: usize = 640;
const CRT_HEIGHT: usize = 240;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CrtTrialCounters {
    flips: u16,
    underruns: u16,
    timeouts: u16,
}

impl CrtTrialCounters {
    fn from_status(status: crate::fpga::LatchedFbufStatus) -> Self {
        Self {
            flips: status.flip_count,
            underruns: status.underrun_count,
            timeouts: status.timeout_count,
        }
    }

    fn delta(self, before: Self) -> Self {
        Self {
            flips: self.flips.wrapping_sub(before.flips),
            underruns: self.underruns.wrapping_sub(before.underruns),
            timeouts: self.timeouts.wrapping_sub(before.timeouts),
        }
    }
}

pub(super) fn run_crt_trial_loop(
    secs: u64,
    ui: &UiDisplay,
    hardware: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
) {
    if secs != CRT_TRIAL_SECS || ui.render_w() != CRT_WIDTH || ui.render_h() != CRT_HEIGHT {
        crate::ui_errln!(
            "crt_trial_status_v1 schema=1 ok=0 reason=invalid-contract requested_secs={} geometry={}x{}",
            secs,
            ui.render_w(),
            ui.render_h()
        );
        restore_hdmi(hardware);
        return;
    }

    let before = hardware
        .read_magik_latched_fbuf_status()
        .ok()
        .map(CrtTrialCounters::from_status)
        .unwrap_or(CrtTrialCounters {
            flips: 0,
            underruns: 0,
            timeouts: 0,
        });
    let started = Instant::now();
    let mut frames = 0u64;
    let mut frame = vec![Rgb565Pixel(0); CRT_WIDTH * CRT_HEIGHT];
    let full_damage = DirtyRectList::from_one(DirtyRect {
        x0: 0,
        y0: 0,
        x1: CRT_WIDTH,
        y1: CRT_HEIGHT,
    });
    let mut presenter = match FpgaVblankLatchHiddenPresenter::open(ui) {
        Ok(presenter) => presenter,
        Err(failure) => {
            crate::ui_errln!(
                "crt_trial_status_v1 schema=1 ok=0 reason=presenter-open stage={} detail={}",
                failure.stage.code(),
                safe_field(&failure.detail)
            );
            restore_hdmi(hardware);
            return;
        }
    };
    let mut failure = None;
    while started.elapsed() < Duration::from_secs(CRT_TRIAL_SECS) {
        render_crt_trial_frame(&mut frame, frames);
        let plan = LauncherFramePlan::new(full_damage, None, None, None, None);
        if let Err(error) = presenter.present_cached_full_frame(
            CachedFrameView::new(&frame, CRT_WIDTH, CRT_HEIGHT),
            plan,
            hardware,
            display_session,
            |_hidden, _plan| Ok(()),
        ) {
            failure = Some(format!("{}-{}", error.stage.code(), error.reason_code()));
            break;
        }
        frames += 1;
        std::thread::sleep(Duration::from_millis(16));
    }

    let active = hardware.read_magik_latched_fbuf_status().ok();
    let after = active
        .map(CrtTrialCounters::from_status)
        .unwrap_or(before)
        .delta(before);
    let fallback = active.is_none_or(|status| {
        status.active_output_route() != crate::fpga::LatchedOutputRoute::Crt240p60
            || (status.reader_flags & 0x000e) != 0
    });
    restore_hdmi(hardware);
    crate::ui_logln!(
        "crt_trial_status_v1 schema=1 ok={} duration_ms={} frames={} flips={} underruns={} timeouts={} fallback={} reason={}",
        u8::from(failure.is_none() && after.underruns == 0 && after.timeouts == 0),
        started.elapsed().as_millis(),
        frames,
        after.flips,
        after.underruns,
        after.timeouts,
        u8::from(fallback),
        failure.as_deref().unwrap_or("none")
    );
}

fn restore_hdmi(hardware: &mut Fpga) {
    let Ok(status) = hardware.read_magik_latched_fbuf_status() else {
        return;
    };
    if status.active_base == 0 || status.active_width == 0 || status.active_height == 0 {
        return;
    }
    let sequence = status
        .active_sequence
        .max(status.pending_sequence)
        .wrapping_add(1)
        .max(1);
    let route =
        LauncherFramebufferRoute::for_scan(status.active_width, status.active_height, false);
    let geometry = crate::fpga::LatchedFbufGeometry::new(status.active_width, route.mode(), 0);
    let _ = hardware.post_magik_latched_fbuf_rgb565(
        sequence,
        status.active_base,
        status.active_width,
        status.active_height,
        geometry,
        crate::fpga::LatchedOutputRoute::Hdmi,
    );
    std::thread::sleep(Duration::from_millis(40));
}

fn render_crt_trial_frame(dst: &mut [Rgb565Pixel], frame: u64) {
    debug_assert_eq!(dst.len(), CRT_WIDTH * CRT_HEIGHT);
    const BARS: [u16; 8] = [
        0xffff, 0xffe0, 0x07ff, 0x07e0, 0xf81f, 0xf800, 0x001f, 0x0000,
    ];
    for y in 0..CRT_HEIGHT {
        for x in 0..CRT_WIDTH {
            let value = if y < 112 {
                BARS[(x * BARS.len() / CRT_WIDTH).min(BARS.len() - 1)]
            } else if x % 16 == 0 || y % 16 == 0 {
                0x8410
            } else {
                0x1082
            };
            dst[y * CRT_WIDTH + x] = Rgb565Pixel(value);
        }
    }
    let marker_x = ((frame * 5) % CRT_WIDTH as u64) as usize;
    for y in 0..CRT_HEIGHT {
        for dx in 0..3 {
            dst[y * CRT_WIDTH + (marker_x + dx) % CRT_WIDTH] = Rgb565Pixel(0xffff);
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
    fn pattern_contains_exact_bars_grid_and_moving_marker() {
        let mut first = vec![Rgb565Pixel(0); CRT_WIDTH * CRT_HEIGHT];
        let mut second = first.clone();
        render_crt_trial_frame(&mut first, 0);
        render_crt_trial_frame(&mut second, 1);

        assert_eq!(first[10 * CRT_WIDTH + 100].0, 0xffe0);
        assert_eq!(first[128 * CRT_WIDTH + 32].0, 0x8410);
        assert_eq!(first[129 * CRT_WIDTH + 33].0, 0x1082);
        assert_eq!(first[50 * CRT_WIDTH].0, 0xffff);
        assert_eq!(second[50 * CRT_WIDTH + 5].0, 0xffff);
        assert_ne!(first, second);
    }

    #[test]
    fn counter_deltas_wrap_without_panicking() {
        let before = CrtTrialCounters {
            flips: u16::MAX,
            underruns: 4,
            timeouts: 9,
        };
        let after = CrtTrialCounters {
            flips: 1,
            underruns: 5,
            timeouts: 9,
        };
        assert_eq!(
            after.delta(before),
            CrtTrialCounters {
                flips: 2,
                underruns: 1,
                timeouts: 0,
            }
        );
    }
}
