// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Matched production sort with allocation accounting confined to this consumer.
use mister_magik_catalog::catalog_sort::sort_ascii_titles;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

#[derive(Clone, Copy, Default)]
struct Counts {
    allocations: usize,
    allocated: usize,
    live: usize,
    peak: usize,
}
thread_local! { static COUNTS: Cell<Option<Counts>> = const { Cell::new(None) }; }
struct Allocator;
fn charge(add: usize, remove: usize, allocation: bool) {
    let _ = COUNTS.try_with(|slot| {
        if let Some(mut c) = slot.get() {
            c.allocations += usize::from(allocation);
            c.allocated += add;
            c.live = c.live.saturating_sub(remove) + add;
            c.peak = c.peak.max(c.live);
            slot.set(Some(c));
        }
    });
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            charge(layout.size(), 0, true);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            charge(layout.size(), 0, true);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        charge(0, layout.size(), false);
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(p, layout, size) };
        if !next.is_null() {
            charge(size, layout.size(), true);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
const SIZES: [usize; 3] = [1_000, 10_000, 50_000];
const FIXTURE: &str = "91f997d2748273b9e663dd9d786fc13d6d59632c928ff67b7514bca55692086c";
#[derive(Clone, PartialEq, Eq)]
struct Row {
    title: String,
    key: String,
    ordinal: usize,
}
fn fixture(count: usize) -> Vec<Row> {
    let mut rows: Vec<_> = (0..count)
        .map(|i| Row {
            title: format!(
                "{} {:03}",
                ["ALPHA", "alpha", "Éclair", "éclair", "Σ", "σ"][i % 6],
                i % 97
            ),
            key: format!("key-{}", i % 19),
            ordinal: i,
        })
        .collect();
    let mut state = 0x24b712cau64;
    for i in (1..count).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        rows.swap(i, state as usize % (i + 1));
    }
    rows
}
fn sort(rows: &mut [Row]) {
    sort_ascii_titles(rows, |r| &r.title, |r| &r.key)
}
pub(super) fn run() -> Result<serde_json::Value, String> {
    let fixtures = SIZES.map(fixture);
    let expected = fixtures.each_ref().map(|rows| {
        let mut rows = rows.clone();
        rows.sort_by(|a, b| {
            a.title
                .to_ascii_lowercase()
                .cmp(&b.title.to_ascii_lowercase())
                .then_with(|| a.key.cmp(&b.key))
        });
        rows
    });
    let mut mechanisms = Vec::new();
    for (i, original) in fixtures.iter().enumerate() {
        let mut rows = original.clone();
        let mut visits = 0;
        COUNTS.with(|slot| slot.set(Some(Counts::default())));
        sort_ascii_titles(
            &mut rows,
            |r| {
                visits += 1;
                &r.title
            },
            |r| &r.key,
        );
        let c = COUNTS.with(|slot| slot.replace(None).unwrap());
        if rows != expected[i] || c.live != 0 {
            return Err("sort parity or temporary ownership failed".into());
        }
        mechanisms.push(serde_json::json!({"rows":rows.len(),"normalizations":visits,"allocations":c.allocations,"allocated_bytes":c.allocated,"peak_temporary_bytes":c.peak,"live_temporary_bytes_after":c.live}));
    }
    let work_count = SIZES.iter().sum::<usize>();
    let mut samples = Vec::new();
    for repetition in 0..2 {
        let mut cases = Vec::new();
        let mut duration_ns = 0u64;
        for (i, original) in fixtures.iter().enumerate() {
            let mut rows = original.clone();
            let start = Instant::now();
            sort(&mut rows);
            let ns = start.elapsed().as_nanos() as u64;
            std::hint::black_box(&rows);
            if rows != expected[i] {
                return Err("timed sort parity failed".into());
            }
            duration_ns += ns;
            cases.push(serde_json::json!({"rows":rows.len(),"duration_ns":ns}));
        }
        samples.push(serde_json::json!({"repetition":repetition,"fixture_identity":FIXTURE,"work_count":work_count,"duration_ns":duration_ns,"ns_per_pixel":duration_ns as f64/work_count as f64,"cases":cases}));
    }
    let sha = std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256")
        .map_err(|_| "native artifact identity missing")?;
    Ok(
        serde_json::json!({"schema_version":1,"workload":"catalog-sort","mode":"timing","artifact_sha256":sha,"correctness":"passed","work_count":work_count,
        "fixture":{"identity":FIXTURE,"sizes":SIZES,"seed":"24b712ca","ordering":"stable ASCII lowercase title then raw stable key","timed_work":"production sort only; allocation observations and exact whole-row oracle outside timing","allocator":"same consumer System adapter on both revisions; accounting disabled during timing"},"mechanisms":mechanisms,"samples":samples}),
    )
}
/// Consumer-only matched accounting; excludes reporter allocations and restores
/// a previous observation when unwinding.
pub(super) fn observe_allocations<R>(f: impl FnOnce() -> R) -> (R, serde_json::Value) {
    struct Restore(Option<Counts>);
    impl Drop for Restore {
        fn drop(&mut self) {
            COUNTS.set(self.0);
        }
    }
    let restore = Restore(COUNTS.replace(Some(Counts::default())));
    let result = f();
    let counts = COUNTS.get().unwrap();
    drop(restore);
    (
        result,
        serde_json::json!({"allocations":counts.allocations,"allocated_bytes":counts.allocated,"peak_temporary_bytes":counts.peak,"final_observed_bytes":counts.live}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allocation_scope_releases_owned_sort_keys() {
        let mut rows = fixture(1_000);
        COUNTS.with(|slot| slot.set(Some(Counts::default())));
        sort(&mut rows);
        let c = COUNTS.with(|slot| slot.replace(None).unwrap());
        assert!(c.allocations > 0);
        assert!(c.peak > 0);
        assert_eq!(c.live, 0);
    }
}
