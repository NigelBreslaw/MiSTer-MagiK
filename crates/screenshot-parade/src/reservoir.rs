// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Strict three-buffer render reservoir shared by production and scene labs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::time::Duration;

pub const STRICT_RENDER_BUFFER_COUNT: usize = 3;
pub const STRICT_READY_CAPACITY: usize = 2;

const READY_FULL_WAIT: Duration = Duration::from_micros(250);

#[derive(Debug)]
pub struct StrictReadyFrame<T> {
    pub tick: u64,
    pub payload: T,
}

#[derive(Debug)]
pub enum StrictFramePoll<T> {
    Frame(StrictReadyFrame<T>),
    Empty,
    Disconnected,
    SequenceFailure {
        expected_tick: u64,
        frame: StrictReadyFrame<T>,
    },
}

#[derive(Debug)]
pub enum StrictFreeBufferPoll<T> {
    Buffer(T),
    Timeout,
    Disconnected,
}

pub struct StrictFrameProducer<B, F> {
    free_rx: Receiver<B>,
    ready_tx: SyncSender<StrictReadyFrame<F>>,
    cancelled: Arc<AtomicBool>,
    ready_depth: Arc<AtomicUsize>,
}

impl<B, F> StrictFrameProducer<B, F> {
    pub fn take_free_timeout(&self, timeout: Duration) -> StrictFreeBufferPoll<B> {
        match self.free_rx.recv_timeout(timeout) {
            Ok(buffer) => StrictFreeBufferPoll::Buffer(buffer),
            Err(RecvTimeoutError::Timeout) => StrictFreeBufferPoll::Timeout,
            Err(RecvTimeoutError::Disconnected) => StrictFreeBufferPoll::Disconnected,
        }
    }

    pub fn publish(&self, mut frame: StrictReadyFrame<F>) -> bool {
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return false;
            }
            self.ready_depth.fetch_add(1, Ordering::AcqRel);
            match self.ready_tx.try_send(frame) {
                Ok(()) => return true,
                Err(TrySendError::Full(returned)) => {
                    self.ready_depth.fetch_sub(1, Ordering::AcqRel);
                    frame = returned;
                    std::thread::park_timeout(READY_FULL_WAIT);
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.ready_depth.fetch_sub(1, Ordering::AcqRel);
                    return false;
                }
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub struct StrictFrameConsumer<B, F> {
    free_tx: SyncSender<B>,
    ready_rx: Receiver<StrictReadyFrame<F>>,
    cancelled: Arc<AtomicBool>,
    ready_depth: Arc<AtomicUsize>,
    expected_tick: u64,
    sequence_failures: u64,
    starvations: u64,
}

impl<B, F> StrictFrameConsumer<B, F> {
    pub fn try_next(&mut self) -> StrictFramePoll<F> {
        match self.ready_rx.try_recv() {
            Ok(frame) => self.validate_frame(frame),
            Err(TryRecvError::Empty) => StrictFramePoll::Empty,
            Err(TryRecvError::Disconnected) => StrictFramePoll::Disconnected,
        }
    }

    fn validate_frame(&mut self, frame: StrictReadyFrame<F>) -> StrictFramePoll<F> {
        self.ready_depth.fetch_sub(1, Ordering::AcqRel);
        if frame.tick != self.expected_tick {
            self.sequence_failures = self.sequence_failures.saturating_add(1);
            return StrictFramePoll::SequenceFailure {
                expected_tick: self.expected_tick,
                frame,
            };
        }
        self.expected_tick = self.expected_tick.saturating_add(1);
        StrictFramePoll::Frame(frame)
    }

    pub fn recycle(&self, buffer: B) -> bool {
        self.free_tx.try_send(buffer).is_ok()
    }

    pub fn ready_depth(&self) -> usize {
        self.ready_depth.load(Ordering::Acquire)
    }

    pub const fn expected_tick(&self) -> u64 {
        self.expected_tick
    }

    pub const fn sequence_failures(&self) -> u64 {
        self.sequence_failures
    }

    pub const fn starvations(&self) -> u64 {
        self.starvations
    }

    pub fn record_starvation(&mut self) {
        self.starvations = self.starvations.saturating_add(1);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl<B, F> Drop for StrictFrameConsumer<B, F> {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn strict_render_reservoir<B, F>(
    buffers: [B; STRICT_RENDER_BUFFER_COUNT],
    first_tick: u64,
) -> (StrictFrameProducer<B, F>, StrictFrameConsumer<B, F>) {
    let (free_tx, free_rx) = sync_channel(STRICT_RENDER_BUFFER_COUNT);
    let (ready_tx, ready_rx) = sync_channel(STRICT_READY_CAPACITY);
    for buffer in buffers {
        free_tx
            .send(buffer)
            .expect("new strict render reservoir owns its free receiver");
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let ready_depth = Arc::new(AtomicUsize::new(0));
    (
        StrictFrameProducer {
            free_rx,
            ready_tx,
            cancelled: Arc::clone(&cancelled),
            ready_depth: Arc::clone(&ready_depth),
        },
        StrictFrameConsumer {
            free_tx,
            ready_rx,
            cancelled,
            ready_depth,
            expected_tick: first_tick,
            sequence_failures: 0,
            starvations: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservoir_preserves_exact_ticks_and_recycles_three_buffers() {
        let (producer, mut consumer) = strict_render_reservoir([10, 20, 30], 7);
        let first = match producer.take_free_timeout(Duration::ZERO) {
            StrictFreeBufferPoll::Buffer(buffer) => buffer,
            other => panic!("expected a free buffer, got {other:?}"),
        };
        assert!(producer.publish(StrictReadyFrame {
            tick: 7,
            payload: first,
        }));
        let frame = match consumer.try_next() {
            StrictFramePoll::Frame(frame) => frame,
            other => panic!("expected a ready frame, got {other:?}"),
        };
        assert_eq!(frame.tick, 7);
        assert_eq!(consumer.expected_tick(), 8);
        assert!(consumer.recycle(frame.payload));
    }

    #[test]
    fn reservoir_reports_sequence_failure_without_advancing() {
        let (producer, mut consumer) = strict_render_reservoir([1, 2, 3], 4);
        let buffer = match producer.take_free_timeout(Duration::ZERO) {
            StrictFreeBufferPoll::Buffer(buffer) => buffer,
            other => panic!("expected a free buffer, got {other:?}"),
        };
        assert!(producer.publish(StrictReadyFrame {
            tick: 5,
            payload: buffer,
        }));
        match consumer.try_next() {
            StrictFramePoll::SequenceFailure {
                expected_tick,
                frame,
            } => {
                assert_eq!(expected_tick, 4);
                assert_eq!(frame.tick, 5);
                assert!(consumer.recycle(frame.payload));
            }
            other => panic!("expected a sequence failure, got {other:?}"),
        }
        assert_eq!(consumer.expected_tick(), 4);
        assert_eq!(consumer.sequence_failures(), 1);
    }

    #[test]
    fn reservoir_records_explicit_starvation_and_cancellation() {
        let (producer, mut consumer) = strict_render_reservoir::<_, i32>([1, 2, 3], 0);
        assert!(matches!(consumer.try_next(), StrictFramePoll::Empty));
        consumer.record_starvation();
        assert_eq!(consumer.starvations(), 1);
        consumer.cancel();
        assert!(producer.is_cancelled());
    }
}
