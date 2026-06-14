# Profile Library Scan Validation

Date: 2026-06-12

Benchmark command:

```bash
scripts/bench-library.sh LIB-SCAN-PROFILE-20260612 --replace-label --iterations 1 --post-reboot
```

Baseline label for comparison: `LIB-SCAN-BASELINE-20260612`.

Results appended to `history/toolchain-bench/results-library.tsv`:

| Label | Scenario | Time |
| --- | --- | ---: |
| `LIB-SCAN-BASELINE-20260612` | cold scan | 57.395 s |
| `LIB-SCAN-PROFILE-20260612` | cold scan | 49.635 s |
| `LIB-SCAN-BASELINE-20260612` | cached arcade load | 0.973 s |
| `LIB-SCAN-PROFILE-20260612` | cached arcade load | 0.740 s |
| `LIB-SCAN-BASELINE-20260612` | post-reboot rescan | 73.110 s |
| `LIB-SCAN-PROFILE-20260612` | post-reboot rescan | 76.939 s |

Catalog validation from `/media/fat/mister-magik/library-scan-bench.sqlite3`:

- `launch_plans` by kind: `mgl=4904`, `mra=2557`, `virtual-mgl=1066`.
- Saturn virtual plans: `152`.
- Top systems included `arcade=2687`, `snes=1569`, `megadrive=779`,
  `gba=772`, `nes=737`, `launcher=679`, `gbc=598`, `n64=296`,
  `gamegear=256`, `saturn=152`, `psx=2`.

Device launch validation:

- Existing MGL accepted by Main FIFO:
  `/media/fat/_DOS Games/7th Guest (MT-32).mgl`.
- Generated Saturn MGL accepted by Main FIFO:
  `/media/fat/mister-magik/launch-cache/validate-saturn.mgl`, mounting
  `/media/fat/games/Saturn/sega-saturn-roms-a-d-chd/Akumajou Dracula X - Gekka no Yasoukyoku PT-BR V1.0.chd`.
- Device was rebooted after each validation launch and returned with
  `mister-magik-fb`, `MiSTer_MagiK`, and `/dev/MiSTer_cmd` present.

## Compressed Set Follow-Up

After adding profile-scoped compressed set support, schema `13` rebuilt on the
MiSTer with:

- `normal_files=26585`, `containers=155`, `entries=281`,
  `discoveries=10088`.
- Full device refresh: `scan_us=51952828`, `import_us=42875639`.
- No-change refresh after rebuild: `skipped=true`, `scan_us=15703382`.

Final catalog counts from `/media/fat/mister-magik/library.sqlite3`:

| System | Count |
| --- | ---: |
| `amiga` | 1562 |
| `neogeo` | 281 |
| `saturn` | 152 |
| `arcade` | 2687 |

Compressed sources verified:

- AmigaVision:
  `catalog-entry|amiga|1561`, launch ref
  `/media/fat/games/Amiga/AmigaVision-MiSTer-2026.04.26.7z`.
- NeoGeo:
  `payloads profile_id='neogeo' entry_path IS NOT NULL` -> `281` ZIP64
  `.neo` entries from `/media/fat/games/NEOGEO/Neo Geo Mister FGPA Ultra Pack.zip`.
  Existing organizer MGLs cover those payloads, so final launch plans prefer the
  `mgl` launchers instead of generating duplicate virtual plans.

## Changed Refresh Shortcut Follow-Up

Date: 2026-06-14

The cached-database changed-refresh path used to do a full directory manifest
walk to prove the DB was stale, then immediately start the real library scan and
walk the same roots again. The refresh path now checks stored directory metadata
first. If a directory is missing, stops being a directory, or has changed
size/mtime, refresh skips the proof walk and goes straight to the rebuild. If
directory metadata is unchanged, it still rebuilds and compares child signatures
to catch same-second edits.

Host fixture benchmark command shape:

```bash
MISTER_LIBRARY_ROOTS=/tmp/mister-library-bench-after2 \
MISTER_LIBRARY_BENCH_SQLITE=/tmp/mister-library-bench-after2.sqlite3 \
MISTER_LIBRARY_BENCH_LABEL=AFTER \
MISTER_LIBRARY_BENCH_ITERATIONS=5 \
MISTER_LIBRARY_BENCH_CHANGED_REFRESH=1 \
cargo run --manifest-path magik-gui/catalog/Cargo.toml --bin library-scan-bench
```

Fixture: 1,300 candidate files, existing SQLite DB, then one synthetic `.nes`
candidate added before each changed-refresh measurement.

| Metric | Before avg | After avg | Change |
| --- | ---: | ---: | ---: |
| `changed_refresh scan_us` | 28.8 ms | 18.8 ms | 34.9% faster |
| `changed_refresh` total | 97.2 ms | 81.9 ms | 15.8% faster |

`MISTER_LIBRARY_BENCH_CHANGED_REFRESH=1` mutates the first configured library
root and should only be used on disposable benchmark roots, not directly on
`/media/fat`.
