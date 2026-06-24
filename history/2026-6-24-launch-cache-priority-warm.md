# Launch Cache Priority Warm Evidence

Item: Preserve and warm launch cache.

Commit candidate cause: virtual launch cache stamp hits only proved the plan set
was unchanged. They did not prove that the generated `.mgl` cache files still
existed, so a missing visible Neo Geo virtual launch file could fall through to
hot launch-path materialization on `/media/fat` and cost about 266 ms.

Fix summary:

- Stamp schema v2 records generated basename, generated content hash, and
  content length per virtual launch plan.
- Matching full-cache stamps now repair missing or wrong-sized generated files
  with metadata checks instead of trusting the stamp alone.
- A low-priority priority warmer runs before the delayed full cache pass and
  materializes the first virtual refs for Neo Geo, Saturn, and SNES.
- `profile-launch-prep.sh` can run `priority-prewarm` and can pass through
  benchmark filters so AI agents can isolate production virtual-launch metrics.

Artifacts:

- `history/toolchain-bench/results-launch-prep.tsv`

BEFORE:

```text
scripts/profile-launch-prep.sh ITEM15-BEFORE-launch-cache-cold --replace-label --scenario cold --iterations 5
scripts/profile-launch-prep.sh ITEM15-BEFORE-launch-cache-warm --replace-label --scenario warm --iterations 10
```

Neo Geo virtual samples:

```text
ITEM15-BEFORE-launch-cache-cold n=40 p50_us=266375 p95_us=283669 max_us=290016
ITEM15-BEFORE-launch-cache-warm n=80 p50_us=74 p95_us=236 max_us=317
```

AFTER:

```text
MISTER_LAUNCH_PREP_VIRTUAL_SYSTEMS=neogeo MISTER_LAUNCH_PREP_VIRTUAL_LIMIT=8 MISTER_LAUNCH_PREP_AMIGAVISION_LIMIT=0 MISTER_LAUNCH_CACHE_PRIORITY_SYSTEMS=neogeo MISTER_LAUNCH_CACHE_PRIORITY_PER_SYSTEM=8 scripts/profile-launch-prep.sh ITEM15-AFTER-launch-cache-priority-neogeo --replace-label --scenario priority-prewarm --iterations 5
MISTER_LAUNCH_PREP_VIRTUAL_SYSTEMS=neogeo MISTER_LAUNCH_PREP_VIRTUAL_LIMIT=8 MISTER_LAUNCH_PREP_AMIGAVISION_LIMIT=0 scripts/profile-launch-prep.sh ITEM15-AFTER-launch-cache-warm-neogeo --replace-label --scenario warm --iterations 10
```

Neo Geo virtual samples:

```text
ITEM15-AFTER-launch-cache-priority-neogeo n=40 p50_us=74 p95_us=175 max_us=192
ITEM15-AFTER-launch-cache-warm-neogeo n=80 p50_us=81 p95_us=222 max_us=247
```

Priority prewarm rows restored the eight sampled Neo Geo files before launch
prep:

```text
ITEM15-AFTER-launch-cache-priority-neogeo priority-prewarm prewarm 0 status=ok prewarm_us=707279 write_bytes=32768 wchar=2500 total=8 written=8 unchanged=0 errors=0
ITEM15-AFTER-launch-cache-priority-neogeo priority-prewarm prewarm 1 status=ok prewarm_us=712082 write_bytes=32768 wchar=2500 total=8 written=8 unchanged=0 errors=0
ITEM15-AFTER-launch-cache-priority-neogeo priority-prewarm prewarm 2 status=ok prewarm_us=727966 write_bytes=32768 wchar=2500 total=8 written=8 unchanged=0 errors=0
ITEM15-AFTER-launch-cache-priority-neogeo priority-prewarm prewarm 3 status=ok prewarm_us=686761 write_bytes=53248 wchar=2500 total=8 written=8 unchanged=0 errors=0
ITEM15-AFTER-launch-cache-priority-neogeo priority-prewarm prewarm 4 status=ok prewarm_us=658276 write_bytes=32768 wchar=2500 total=8 written=8 unchanged=0 errors=0
```

Validation:

```text
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features virtual_launch -- --nocapture
scripts/test-host-tools.sh
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```

