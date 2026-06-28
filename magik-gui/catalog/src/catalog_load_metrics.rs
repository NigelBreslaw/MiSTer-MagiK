use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CatalogLoadCounters {
    pub sqlite_opens: u64,
    pub summary_reads: u64,
    pub nav_projection_reads: u64,
    pub worker_cache_loads: u64,
    pub ui_catalog_loads: u64,
}

static SQLITE_OPENS: AtomicU64 = AtomicU64::new(0);
static SUMMARY_READS: AtomicU64 = AtomicU64::new(0);
static NAV_PROJECTION_READS: AtomicU64 = AtomicU64::new(0);
static WORKER_CACHE_LOADS: AtomicU64 = AtomicU64::new(0);
static UI_CATALOG_LOADS: AtomicU64 = AtomicU64::new(0);

pub fn reset() {
    SQLITE_OPENS.store(0, Ordering::Relaxed);
    SUMMARY_READS.store(0, Ordering::Relaxed);
    NAV_PROJECTION_READS.store(0, Ordering::Relaxed);
    WORKER_CACHE_LOADS.store(0, Ordering::Relaxed);
    UI_CATALOG_LOADS.store(0, Ordering::Relaxed);
}

pub fn snapshot() -> CatalogLoadCounters {
    CatalogLoadCounters {
        sqlite_opens: SQLITE_OPENS.load(Ordering::Relaxed),
        summary_reads: SUMMARY_READS.load(Ordering::Relaxed),
        nav_projection_reads: NAV_PROJECTION_READS.load(Ordering::Relaxed),
        worker_cache_loads: WORKER_CACHE_LOADS.load(Ordering::Relaxed),
        ui_catalog_loads: UI_CATALOG_LOADS.load(Ordering::Relaxed),
    }
}

pub fn record_sqlite_open() {
    SQLITE_OPENS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_summary_read() {
    SUMMARY_READS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_nav_projection_read() {
    NAV_PROJECTION_READS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_worker_cache_load() {
    WORKER_CACHE_LOADS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_ui_catalog_load() {
    UI_CATALOG_LOADS.fetch_add(1, Ordering::Relaxed);
}

pub fn format_snapshot(counters: CatalogLoadCounters) -> String {
    format!(
        "sqlite_opens={} summary_reads={} nav_projection_reads={} worker_cache_loads={} ui_catalog_loads={}",
        counters.sqlite_opens,
        counters.summary_reads,
        counters.nav_projection_reads,
        counters.worker_cache_loads,
        counters.ui_catalog_loads
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment_snapshot_and_format_in_stable_order() {
        let before = snapshot();
        record_sqlite_open();
        record_sqlite_open();
        record_summary_read();
        record_nav_projection_read();
        record_worker_cache_load();
        record_ui_catalog_load();

        let after = snapshot();
        assert!(after.sqlite_opens >= before.sqlite_opens + 2);
        assert!(after.summary_reads >= before.summary_reads + 1);
        assert!(after.nav_projection_reads >= before.nav_projection_reads + 1);
        assert!(after.worker_cache_loads >= before.worker_cache_loads + 1);
        assert!(after.ui_catalog_loads >= before.ui_catalog_loads + 1);

        assert_eq!(
            format_snapshot(CatalogLoadCounters {
                sqlite_opens: 2,
                summary_reads: 1,
                nav_projection_reads: 1,
                worker_cache_loads: 1,
                ui_catalog_loads: 1,
            }),
            "sqlite_opens=2 summary_reads=1 nav_projection_reads=1 worker_cache_loads=1 ui_catalog_loads=1"
        );
    }
}
