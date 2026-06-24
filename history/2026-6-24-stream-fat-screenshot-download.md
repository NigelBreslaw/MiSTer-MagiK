# Stream FAT Screenshot Download Experiment

Goal: test whether screenshot pack downloads get faster when the benchmark
streams the raw `.mmlz4b` object directly into a hidden `/media/fat` temp file
while hashing the same bytes through `sha256sum`, instead of downloading to
`/tmp`, hashing, then copying to FAT.

The runtime path was not changed. The experiment adds
`media-bench-download --save-strategy staged|stream-fat` and
`scripts/profile-screenshot-download.sh --save-strategy staged|stream-fat`.

## Device Runs

All runs used the deployed `release-device` binary on the MiSTer and
`--prime-cache --replace-label`. Each suite below has 10 measured rows, all
`bench-ok`, excluding the prime-cache row. The TSV also keeps one earlier
successful NeoGeo stream-fat smoke row.

```text
scripts/profile-screenshot-download.sh STREAM-STAGED-NEOGEO-20260624 --system neogeo --iterations 10 --prime-cache --replace-label --save-strategy staged
scripts/profile-screenshot-download.sh STREAM-FAT-NEOGEO-20260624 --system neogeo --iterations 10 --prime-cache --replace-label --save-strategy stream-fat
scripts/profile-screenshot-download.sh STREAM-STAGED-ARCADE-20260624 --system arcade --iterations 10 --prime-cache --replace-label --save-strategy staged
scripts/profile-screenshot-download.sh STREAM-FAT-ARCADE-20260624 --system arcade --iterations 10 --prime-cache --replace-label --save-strategy stream-fat
scripts/profile-screenshot-download.sh STREAM-STAGED-SATURN-20260624 --system saturn --iterations 10 --prime-cache --replace-label --save-strategy staged
scripts/profile-screenshot-download.sh STREAM-FAT-SATURN-20260624 --system saturn --iterations 10 --prime-cache --replace-label --save-strategy stream-fat
```

## Results

```text
system   strategy    bytes     total median  total mean  total p95/max  verify median  save median
neogeo   staged      24283092  8279 ms       8381 ms     8936 ms        1114 ms        1922 ms
neogeo   stream-fat  24283092  6574 ms       6587 ms     7141 ms        2 ms           69 ms
arcade   staged      34623433  11736 ms      11778 ms    12302 ms       1585 ms        2791 ms
arcade   stream-fat  34623433  9088 ms       9068 ms     9797 ms        2 ms           90 ms
saturn   staged      10979806  3939 ms       3959 ms     4093 ms        513 ms         884 ms
saturn   stream-fat  10979806  3184 ms       3200 ms     3357 ms        2 ms           42 ms
```

Median total improvement:

```text
neogeo: 8279 ms -> 6574 ms, 20.6% faster
arcade: 11736 ms -> 9088 ms, 22.6% faster
saturn: 3939 ms -> 3184 ms, 19.2% faster
```

## Notes

- The first stream-fat smoke attempt accidentally wrote the `wget -S` stderr
  progress file beside the benchmark artifact on `/media/fat`, making the
  download crawl. Moving that headers/progress capture back to `/tmp` fixed the
  issue.
- Stream-fat shifts time from separate `verify_ms` and `save_ms` into
  `download_ms` because bytes are hashed and written to FAT while being
  received.
- `stream-fat` meets the experiment threshold: median total time improved by at
  least 15% on all three systems, and p95/max did not materially worsen.

## Validation

```text
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features media_bench_download
scripts/test-host-tools.sh
cargo fmt --manifest-path magik-gui/Cargo.toml --check
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```

The broader `mister-magik-fb` binary clippy target still reports unrelated
pre-existing warnings outside this benchmark path.

## Recommendation

Promote the streaming design from benchmark experiment to the production
screenshot media downloader, preserving the same safety rule: write only to a
hidden temp, verify size and SHA-256 before rename, then sync/rename/parent-sync
and update state.
