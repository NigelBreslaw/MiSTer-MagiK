# Direct-To-Main Production Performance Fix Results

Date: 2026-06-24

Scope: production MiSTer MagiK performance, benchmark reliability, catalog/media
I/O, launch cache, and supported scene gates. Experimental effects remained out
of scope.

This resolves the direct-to-main plan that followed the production performance
review at commit `9c9ee441`. The work landed as one reviewed commit per item on
`main`; no PR and no squash.

## Commit Map

| Item | Commit | Resolution | Metric / Evidence |
| --- | --- | --- | --- |
| 1. Fix held-scroll benchmark truth | `485d8d60` | Preserved scenario step state until Arcade rows exist and failed zero-motion traces. | `moving_frames 0 -> 3577`, `fractional_visual_index_frames 0 -> 3130`; see `history/2026-6-24-held-scroll-benchmark-truth.md`. |
| 2. Make benchmark failures AI-visible | `865a43c0` | Added structured run/artifact/validity/motion rows, status artifacts, screenshot dimension and nonblank checks. | Smoke run emitted `validity_tsv valid=1`, `artifact_tsv width=960 height=540 nonblank=true`; see `history/2026-6-24-ai-visible-preview-benchmarks.md`. |
| 3. Reduce preview trace overhead | `6aaebffe` | Buffered preview trace rows in memory and deferred TSV formatting/writes. | CPU samples in `write_preview_trace` dropped `4.96% -> 0.11%`; see `history/2026-6-24-preview-trace-overhead.md`. |
| 4. Attribute rare frame spikes | `dc6348cb` | Added slow-frame prepare/detail/status attribution and script summary rows. | Unattributed slow work frames dropped `5 -> 0`; see `history/2026-6-24-slow-frame-attribution.md`. |
| 5. Unify RGB565 preview fade into one production path | `ee7f3cfb` | Removed same-geometry/generic split and routed all production RGB565 fades through one clipping/scaling row path with Rust NEON where available. | `preview_blit_us` p99 dropped `3313us -> 1655us`; generic fade symbol removed from CPU profile; see `history/2026-6-24-rgb565-preview-fade-single-path.md`. |
| 6. Trim Arcade hot-loop churn | `f0c3923e` | Cached row fingerprints, skipped unchanged motion hashing, deduped preview schedule windows, and copied status strings only on due frames. | `arcade_list_update_us` p99 `629us -> 557us`, `preview_schedule_us` avg `177.68us -> 80.86us`, work misses `10 -> 0`; see `history/2026-6-24-arcade-hot-loop-churn.md`. |
| 7. Add cold media UI repro and observability | `831d0661` | Added `profile-media-cold-boot.sh` plus queue/progress/UI visibility rows and artifacts. | Proved arcade/saturn/neogeo all reached discovery, ensure, queue, progress, and terminal states; Neo Geo had `ui_rendered_seen=0`; see `history/2026-6-24-media-cold-boot-observability.md`. |
| 8. Root-cause and fix Neo Geo cold-boot download visibility | `8d9cbb87` | Added standalone media progress popup after catalog scan popup hides, plus terminal hold clearing. | Neo Geo `ui_rendered_seen 0 -> 1`, `ui_issue render_missing -> none`; see `history/2026-6-24-neogeo-media-visibility.md`. |
| 9. Align download benchmark with production save path | `0a156039` | Changed `media-bench-download` to use `publish_pack_file_with_progress` and emit publish/state/cleanup stages. | Stage rows showed `publish_copy=2067ms`, `publish_parent_sync=7ms`, `save_ms=2179ms`; see `history/2026-6-24-download-bench-production-publish.md`. |
| 10. Fix screenshot download save-path performance | `34396d5c` | Confirmed the 8.3s save was benchmark path mismatch, then gated the aligned production path against direct save. | Neo Geo save `8314ms -> 2215ms`, total `15704ms -> 9400ms`, within direct-save threshold `2746ms`; see `history/2026-6-24-screenshot-download-save-path.md`. |
| 11. Batch ZIP central-directory reads | `85e9f37e` | Parsed normal central directories from a bounded 8 MiB buffer and kept a 64 KiB streaming fallback for large directories. | `scan_stage_archive_toc 26.712ms -> 9.351ms`, counts unchanged; see `history/2026-6-24-zip-central-directory-buffering.md`. |
| 12. Cap MGL and header metadata reads | `54475bc5` | Capped `.mgl` reads, reused parsed covered-payload paths, avoided unnecessary Saturn/CHD header probes. | `scan_stage_file_discovery 2161ms -> 2091ms`, `scan_stage_classify_total 2561ms -> 2494ms`, counts unchanged; see `history/2026-6-24-mgl-header-metadata-caps.md`. |
| 13. Remove extra summary/SQLite reload | `a6f169bc` | Built `library.summary.json` and worker-ready catalog from already materialized transaction data instead of reopening SQLite twice. | `library_db_saved -> library_ready 367ms -> 0ms`, warm median `full_catalog_ready_load_us 474661 -> 441342`; see `history/2026-6-24-summary-sqlite-reload-removal.md`. |
| 14. Optimize SQLite metadata/import allocations | `c225c94f` | Loaded MAME/HBMAME metadata only for needed arcade/Neo Geo setnames and reused the preferred discovery set. | `import_stage_metadata_load 1534ms -> 1168ms`, 23.9% reduction, row counts unchanged; see `history/2026-6-24-sqlite-metadata-allocation-filter.md`. |
| 15. Preserve and warm launch cache | `3ee2f179` | Added launch-cache stamp v2 file/hash/length checks, repair-on-stamp-hit, and priority prewarm for visible virtual refs. | Neo Geo virtual cold launch prep `p95 283669us -> 175us`; warm p95 stayed `<3ms`; see `history/2026-6-24-launch-cache-priority-warm.md`. |
| 16. Fix full_motion scene gate | `3088d8e7` | Parsed cumulative vsync fallback/error counters as post-warmup deltas and preserved per-window profiler summing. | Historical `full_motion` gate `vsync_fallback 14 -> 0`; after all-scenes gate passed with `timing_ok=yes visual_ok=yes capture_ok=yes`; see `history/2026-6-24-full-motion-scene-gate.md`. |

## Validation Pattern

Each performance-impact commit recorded immediate-parent before evidence and
candidate after evidence. Each candidate ran the shared gate unless the commit
was documentation/history-only:

```bash
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```

Catalog-touching commits also ran catalog crate clippy and targeted catalog
tests. Script-touching commits ran `scripts/test-host-tools.sh` or the relevant
script self-test.

## Historical Artifacts Committed With This Summary

This history commit also preserves generated benchmark TSV rows from the item
work:

- `history/toolchain-bench/results.tsv`
- `history/toolchain-bench/results-first-scan.tsv`
- `history/toolchain-bench/results-launch-handoff.tsv`
- `history/toolchain-bench/results-launch-prep.tsv`
- `history/toolchain-bench/results-library-io.tsv`
- `history/toolchain-bench/results-library-save.tsv`
- `history/toolchain-bench/results-screenshot-download.tsv`
- `history/toolchain-bench/results-screenshot-save.tsv`
- `history/toolchain-bench/results-warm-catalog.tsv`

The original production review at `9c9ee441` is preserved separately in
`history/2026-6-24-production-performance-review-9c9ee441.md`.
