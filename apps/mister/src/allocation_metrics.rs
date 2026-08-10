// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Thread-local allocation counters used only inside explicitly measured spans.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

pub(crate) struct TrackingAllocator;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AllocationMetrics {
    pub(crate) allocations: u64,
    pub(crate) bytes: u64,
}

pub(crate) fn begin() {
    ALLOCATIONS.set(0);
    BYTES.set(0);
    ACTIVE.set(true);
}

pub(crate) fn finish() -> AllocationMetrics {
    ACTIVE.set(false);
    AllocationMetrics {
        allocations: ALLOCATIONS.get(),
        bytes: BYTES.get(),
    }
}

fn record(bytes: usize) {
    if ACTIVE.get() {
        ALLOCATIONS.set(ALLOCATIONS.get().saturating_add(1));
        BYTES.set(
            BYTES
                .get()
                .saturating_add(bytes.try_into().unwrap_or(u64::MAX)),
        );
    }
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: delegates the exact allocation request to the system allocator.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: delegates the exact allocation request to the system allocator.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds GlobalAlloc's pointer/layout contract.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds GlobalAlloc's pointer/layout contract.
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            record(new_size);
        }
        resized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_measurements_are_empty() {
        let metrics = finish();
        assert_eq!(metrics, AllocationMetrics::default());
    }
}
