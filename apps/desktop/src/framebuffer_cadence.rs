// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

pub const CADENCE_EVENT_CAPACITY: usize = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CadenceEventKind {
    SourceReceived,
    DecodeComplete,
    PixelBufferReady,
    MailboxPublish,
    MailboxReplace,
    MailboxTake,
    UiApplied,
    RedrawRequested,
    DisplayLinkTick,
    RedrawSubmit,
    BeforeRendering,
    AfterRendering,
    ChromeRefresh,
    WindowFocused,
    WindowOccluded,
}

impl CadenceEventKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::SourceReceived => "source-received",
            Self::DecodeComplete => "decode-complete",
            Self::PixelBufferReady => "pixel-buffer-ready",
            Self::MailboxPublish => "mailbox-publish",
            Self::MailboxReplace => "mailbox-replace",
            Self::MailboxTake => "mailbox-take",
            Self::UiApplied => "ui-applied",
            Self::RedrawRequested => "redraw-requested",
            Self::DisplayLinkTick => "display-link-tick",
            Self::RedrawSubmit => "redraw-submit",
            Self::BeforeRendering => "before-rendering",
            Self::AfterRendering => "after-rendering",
            Self::ChromeRefresh => "chrome-refresh",
            Self::WindowFocused => "window-focused",
            Self::WindowOccluded => "window-occluded",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CadenceEvent {
    pub at_us: u64,
    pub kind: CadenceEventKind,
    pub source_sequence: u64,
    pub source_timestamp_us: u64,
    pub applied_serial: u64,
    pub queue_age_us: u64,
    pub value: i64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CadenceSummary {
    pub samples: usize,
    pub interval_p50_us: u64,
    pub interval_p95_us: u64,
    pub interval_p99_us: u64,
    pub interval_max_us: u64,
    pub gaps_over_20ms: usize,
    pub gaps_over_34ms: usize,
    pub max_consecutive_over_20ms: usize,
    pub bucket_500ms_min: usize,
    pub bucket_500ms_max: usize,
}

struct CadenceRing {
    origin: Instant,
    events: VecDeque<CadenceEvent>,
}

impl CadenceRing {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            events: VecDeque::with_capacity(CADENCE_EVENT_CAPACITY),
        }
    }

    fn push_at(&mut self, mut event: CadenceEvent, at: Instant) {
        event.at_us = at.saturating_duration_since(self.origin).as_micros() as u64;
        if self.events.len() == CADENCE_EVENT_CAPACITY {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }
}

pub struct FramebufferCadenceTrace {
    ring: Mutex<CadenceRing>,
}

impl Default for FramebufferCadenceTrace {
    fn default() -> Self {
        Self {
            ring: Mutex::new(CadenceRing::new()),
        }
    }
}

impl FramebufferCadenceTrace {
    pub fn reset(&self) {
        if let Ok(mut ring) = self.ring.lock() {
            *ring = CadenceRing::new();
        }
    }

    pub fn record(
        &self,
        kind: CadenceEventKind,
        source_sequence: u64,
        source_timestamp_us: u64,
        applied_serial: u64,
        queue_age_us: u64,
        value: i64,
    ) {
        self.record_at(
            kind,
            Instant::now(),
            source_sequence,
            source_timestamp_us,
            applied_serial,
            queue_age_us,
            value,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_at(
        &self,
        kind: CadenceEventKind,
        at: Instant,
        source_sequence: u64,
        source_timestamp_us: u64,
        applied_serial: u64,
        queue_age_us: u64,
        value: i64,
    ) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.push_at(
                CadenceEvent {
                    at_us: 0,
                    kind,
                    source_sequence,
                    source_timestamp_us,
                    applied_serial,
                    queue_age_us,
                    value,
                },
                at,
            );
        }
    }

    pub fn events(&self) -> Vec<CadenceEvent> {
        self.ring
            .lock()
            .map(|ring| ring.events.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn summary(&self, kind: CadenceEventKind) -> CadenceSummary {
        summarize_events(&self.events(), kind)
    }

    pub fn write_tsv(&self, path: &Path) -> io::Result<()> {
        let mut output = String::from(
            "at_us\tevent\tsource_sequence\tsource_timestamp_us\tapplied_serial\tqueue_age_us\tvalue\n",
        );
        for event in self.events() {
            let _ = writeln!(
                output,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                event.at_us,
                event.kind.label(),
                event.source_sequence,
                event.source_timestamp_us,
                event.applied_serial,
                event.queue_age_us,
                event.value,
            );
        }
        fs::write(path, output)
    }
}

fn summarize_events(events: &[CadenceEvent], kind: CadenceEventKind) -> CadenceSummary {
    let timestamps = events
        .iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.at_us)
        .collect::<Vec<_>>();
    let mut intervals = timestamps
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]))
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut consecutive = 0usize;
    let mut max_consecutive = 0usize;
    for pair in timestamps.windows(2) {
        if pair[1].saturating_sub(pair[0]) > 20_000 {
            consecutive += 1;
            max_consecutive = max_consecutive.max(consecutive);
        } else {
            consecutive = 0;
        }
    }
    let buckets = complete_bucket_counts(&timestamps, 500_000);
    CadenceSummary {
        samples: timestamps.len(),
        interval_p50_us: percentile(&intervals, 0.50),
        interval_p95_us: percentile(&intervals, 0.95),
        interval_p99_us: percentile(&intervals, 0.99),
        interval_max_us: intervals.last().copied().unwrap_or(0),
        gaps_over_20ms: intervals.iter().filter(|value| **value > 20_000).count(),
        gaps_over_34ms: intervals.iter().filter(|value| **value > 34_000).count(),
        max_consecutive_over_20ms: max_consecutive,
        bucket_500ms_min: buckets.iter().copied().min().unwrap_or(0),
        bucket_500ms_max: buckets.iter().copied().max().unwrap_or(0),
    }
}

fn percentile(values: &[u64], percentile: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let rank = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[rank.min(values.len() - 1)]
}

fn complete_bucket_counts(timestamps: &[u64], width_us: u64) -> Vec<usize> {
    let (Some(first), Some(last)) = (timestamps.first(), timestamps.last()) else {
        return Vec::new();
    };
    let complete = last.saturating_sub(*first) / width_us;
    let mut buckets = vec![0usize; complete as usize];
    for timestamp in timestamps {
        let index = timestamp.saturating_sub(*first) / width_us;
        if let Some(bucket) = buckets.get_mut(index as usize) {
            *bucket += 1;
        }
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(at_us: u64, kind: CadenceEventKind) -> CadenceEvent {
        CadenceEvent {
            at_us,
            kind,
            source_sequence: 0,
            source_timestamp_us: 0,
            applied_serial: 0,
            queue_age_us: 0,
            value: 0,
        }
    }

    #[test]
    fn percentile_uses_nearest_rank_in_sorted_samples() {
        assert_eq!(percentile(&[10, 20, 30, 40, 50], 0.50), 30);
        assert_eq!(percentile(&[10, 20, 30, 40, 50], 0.95), 50);
        assert_eq!(percentile(&[], 0.95), 0);
    }

    #[test]
    fn complete_buckets_exclude_partial_tail() {
        let timestamps = [0, 100_000, 499_999, 500_000, 800_000, 1_000_000];
        assert_eq!(complete_bucket_counts(&timestamps, 500_000), [3, 2]);
    }

    #[test]
    fn summary_detects_pulsed_consecutive_slow_frames() {
        let events = [0, 16_000, 32_000, 56_000, 80_000, 96_000]
            .into_iter()
            .map(|at| event(at, CadenceEventKind::AfterRendering))
            .collect::<Vec<_>>();
        let summary = summarize_events(&events, CadenceEventKind::AfterRendering);
        assert_eq!(summary.gaps_over_20ms, 2);
        assert_eq!(summary.max_consecutive_over_20ms, 2);
        assert_eq!(summary.interval_max_us, 24_000);
    }

    #[test]
    fn ring_keeps_only_newest_events() {
        let mut ring = CadenceRing::new();
        for sequence in 0..CADENCE_EVENT_CAPACITY as u64 + 3 {
            ring.push_at(
                CadenceEvent {
                    at_us: 0,
                    kind: CadenceEventKind::SourceReceived,
                    source_sequence: sequence,
                    source_timestamp_us: 0,
                    applied_serial: 0,
                    queue_age_us: 0,
                    value: 0,
                },
                Instant::now(),
            );
        }
        assert_eq!(ring.events.len(), CADENCE_EVENT_CAPACITY);
        assert_eq!(
            ring.events.front().map(|event| event.source_sequence),
            Some(3)
        );
    }
}
