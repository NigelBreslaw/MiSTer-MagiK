# Return projection phase timing

Benchmark class: correctness-only. No performance claim.

Confirmed cause: `LibraryCatalogLoad::open_us` combined file reading, LZ4
decompression, binary decode/allocation, and validation. That made the 758 ms
value look like exFAT I/O and encouraged an ineffective compressed `/tmp`
cache.

Before:

- Parent: `4786046cf343053daaca8a0248a1f29a2f59eabf`.
- The launch-return trace reported only `open_us=758313` and
  `catalog_us=448441`; the read/decompress/decode split was unavailable.
- The production return gate remained over budget.

After:

- The authoritative bounded projection reader reports file read, decompression,
  and binary decode/allocation separately.
- The same production return path measured:
  - file read: 10,484 us
  - LZ4 decompression: 125,701 us
  - binary decode/allocation: 611,748 us
  - catalog reconstruction: 437,318 us
  - total navigation load: 1,196,974 us
- The result proves file I/O is not the dominant return bottleneck. No
  performance improvement is claimed.

Correctness:

- The reader opens the file once and consumes at most 64 MiB plus one sentinel
  byte, so an oversized replacement is rejected without an unbounded
  allocation.
- Existing callers retain the same `Option<CatalogNavigationProjection>`
  behavior; the timed API is additive.

Validation:

- Navigation projection round-trip and bounded-reader tests.
- `scripts/dev-rust test`: 283 passed.
- `scripts/dev-rust check`.
- GUI library clippy and catalog all-target clippy with `-D warnings`.

Raw evidence:

- `build/launch-return/PROJECTION-PHASES-4786046C-AFTER2.log`

Review:

- Independent reviewer: `/root/review_projection_phase_timing`.
- Reviewed code diff:
  `0f2c6f60186f1deb3668add07c6c94395819f36414ee392f7bed8d8d5592e271`.
- Reviewed evidence:
  `55e72f0b16bbd18ba084bcd3941105bbeec2e7d00e55d250c5b32cd6cece1966`.
- Result: approved with no unresolved actionable findings.
