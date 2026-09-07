// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test-only synchronous I/O attempts on the calling thread, isolated from
//! parallel tests and background loading. Instrumented at current reader boundaries.
use std::cell::Cell;

thread_local! {
    static READ_ATTEMPTS: Cell<u64> = const { Cell::new(0) };
}

pub fn read_attempts() -> u64 {
    READ_ATTEMPTS.get()
}

pub(crate) fn record_read() {
    READ_ATTEMPTS.set(READ_ATTEMPTS.get() + 1);
}

#[cfg(test)]
mod tests {
    #[test]
    fn failed_navpack_and_sqlite_opens_are_observed() {
        let root = crate::test_support::unique_temp_dir("io-metrics-readers");
        let missing = root.join("missing");
        let before = super::read_attempts();
        assert!(crate::navpack::MappedNavPack::open(&missing, 0, "arcade", 1, 0).is_err());
        assert!(super::read_attempts() > before);
        let before = super::read_attempts();
        let id = crate::catalog_classify::SystemId::parse("arcade").unwrap();
        assert!(
            crate::system_shard::open_system_shard(
                &missing,
                &missing,
                &id,
                1,
                crate::shard_registry::production_registry_limits().shard,
            )
            .is_err()
        );
        assert!(super::read_attempts() > before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_lazy_registry_reader_records_io_even_when_open_fails() {
        let before = super::read_attempts();
        let root = crate::test_support::unique_temp_dir("io-metrics");
        let result = crate::lazy_sharded_reader::LazyShardedCatalogReader::open(
            &root,
            crate::shard_registry::production_registry_limits(),
        );
        assert!(result.is_err());
        assert!(super::read_attempts() > before);
        let child = std::thread::spawn(super::read_attempts).join().unwrap();
        assert_eq!(child, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
