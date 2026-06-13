# Preview zstd archive benchmark - 2026-06-13

Goal: compare the current arcade screenshot cache layout, which reads individual
`.rgb565` files from `/media/fat`, against a single compressed file copied to the
MiSTer and read one screenshot at a time.

## Source set

- Local source:
  `build/arcade-screenshot-cache/hybrid-20260613/raw565-hybrid-320x320`
- Files: 910 `.rgb565`
- Raw payload bytes: 147,676,440 bytes (140.84 MiB)
- Decoded pixel bytes reported by validator: 147,658,240 bytes

## Archive formats

The first experiment used a single indexed file with magic `MMZST01\0`. Each
preview is stored as an independent zstd frame, so lookup is still per
screenshot:

1. read the archive index once
2. seek to the selected entry
3. read only that entry's compressed bytes
4. zstd-decompress that one screenshot
5. validate/use the normal `MM56501\0` raw565 payload

This is not a one-time whole-archive decompression model. The decompression cost
is paid for each screenshot read.

Follow-up experiments used LZ4:

- `MMLZ401\0`: one standard LZ4 frame per screenshot.
- `MMLZ4B1\0`: one raw LZ4 block per screenshot, stripped from the Mac-generated
  frame and stored with a tiny raw/compressed flag. This avoids per-screenshot
  LZ4 frame decoder overhead while keeping the same independent random lookup
  model.

Mac-side packer:

```bash
node scripts/build-preview-zstd-archive.mjs \
  build/arcade-screenshot-cache/hybrid-20260613/raw565-hybrid-320x320 \
  build/arcade-screenshot-cache/hybrid-20260613/raw565-hybrid-320x320-zstd9.mmzst \
  zstd 9

node scripts/build-preview-zstd-archive.mjs \
  build/arcade-screenshot-cache/hybrid-20260613/raw565-hybrid-320x320 \
  build/arcade-screenshot-cache/hybrid-20260613/raw565-hybrid-320x320-lz4block-12.mmlz4b \
  lz4-block 12
```

MiSTer-side benchmark binary:

```bash
magik-gui/build-arm.sh --fast-dev --preview-archive-bench
scripts/mister put \
  magik-gui/target/armv7-unknown-linux-gnueabihf/release-fast-dev/preview-archive-bench \
  /media/fat/mister-magik/bench/preview-archive-bench
```

Benchmark command shape:

```bash
MISTER_BENCH_DROP_CACHES=1 \
  /media/fat/mister-magik/bench/preview-archive-bench raw-dir \
  /media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320 5

MISTER_BENCH_DROP_CACHES=1 \
  /media/fat/mister-magik/bench/preview-archive-bench zstd \
  /media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-zstd9.mmzst 5
```

Each trial shuffles all 910 entries and drops Linux page cache before the trial.

## Results

Five-trial averages:

| mode | archive bytes | total ms | avg per screenshot us | p50 us | p95 us | read ms | decompress ms |
|------|---------------|----------|-----------------------|--------|--------|---------|----------------|
| raw `.rgb565` dir | n/a | 8503.7 | 9344 | 9067 | 10746 | 8488.4 | 0.0 |
| zstd level 1 | 20,675,812 | 5245.1 | 5763 | 5041 | 10054 | 1249.8 | 3853.6 |
| zstd level 3 | 18,960,210 | 4969.2 | 5460 | 4747 | 9886 | 1070.0 | 3759.8 |
| zstd level 6 | 17,593,206 | 4741.4 | 5210 | 4388 | 9721 | 1092.0 | 3509.1 |
| zstd level 9 | 16,772,537 | 4565.9 | 5017 | 4101 | 9646 | 1069.7 | 3356.9 |
| LZ4 frame level 4 | 22,609,382 | 3120.1 | 3428 | 2744 | 7323 | 1390.5 | 1713.9 |
| LZ4 frame level 9 | 19,964,757 | 2922.0 | 3210 | 2546 | 7071 | 1270.9 | 1635.2 |
| LZ4 frame level 12 | 19,368,142 | 2846.4 | 3127 | 2507 | 7000 | 1216.4 | 1614.2 |
| LZ4 block level 9 | 19,948,377 | 2010.4 | 2209 | 1509 | 6283 | 1190.2 | 805.9 |
| LZ4 block level 12 | 19,351,762 | 2001.6 | 2199 | 1503 | 6242 | 1183.7 | 802.4 |

All tested zstd archives and LZ4 levels 4+ were under the 25 MB target. LZ4
level 1 was skipped on-device because it missed the target at 29,696,848 bytes.

The best measured point was LZ4 block level 12: about 4.25x faster total time
than the raw directory baseline, with an archive size of about 18.46 MiB.

## Takeaway

The current bottleneck is SD/exFAT read volume and many-file access, not raw565
header validation. Zstd proved the single-file idea, but its per-screenshot
decompression cost was still high. LZ4 block is a much better CPU fit for the
Cortex-A9: it cuts cold shuffled full-set time from about 8.5 s to about 2.0 s
while keeping the archive under 25 MB.

At this point the likely next implementation step looked like replacing
`std::fs::read(cache_path)` in the preview worker with a long-lived LZ4-block
archive handle plus the same per-entry seek/read/decompress path used by the
benchmark. The real-app A/B below changed that conclusion.

## Real app A/B

Follow-up real-app benchmark used the actual optimized `mister-magik-fb ui
arcade` path, built with `magik-gui/build-arm.sh --fast --ui-scope arcade`, and
selected the archive path with:

```bash
MISTER_PREVIEW_ARCHIVE=/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-lz4block-12.mmlz4b
```

This is the result that matters for whether the current app gets faster.

### Held-scroll

Normal continuous arcade browsing workload:

| path | decoded previews | avg load us | p50 load us | p95 load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms |
|------|------------------|-------------|-------------|-------------|-------------|---------------|-------------------|-------------------|------------|
| raw files | 99 | 2125 | 1929 | 3128 | 621 | 1492 | 16352 | 16946 | 1 |
| LZ4 block archive | 99 | 2387 | 2318 | 3235 | 81 | 2295 | 16366 | 16951 | 1 |

### Screenshot-stress

Preview-heavy stress workload:

| path | decoded previews | avg load us | p50 load us | p95 load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms |
|------|------------------|-------------|-------------|-------------|-------------|---------------|-------------------|-------------------|------------|
| raw files | 187 | 2239 | 2242 | 3025 | 806 | 1419 | 16399 | 16976 | 0 |
| LZ4 block archive | 187 | 2400 | 2312 | 3258 | 103 | 2288 | 16385 | 16992 | 0 |

Real-app conclusion: the archive is not a win for the current optimized runtime
path. It is excellent for cold bulk reads, but in the actual app the raw `.rgb565`
files are usually warm enough that their lower CPU cost beats the archive's
smaller reads. Frame timing is effectively unchanged because the scenario is
vsync-bound.

### Turbo-hold with fade

Fast continuous scrolling with the real fade transition:

| path | decoded previews | avg load us | p50 load us | p95 load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders |
|------|------------------|-------------|-------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|
| raw files | 187 | 1938 | 1785 | 2881 | 550 | 1376 | 16367 | 16929 | 1 | 1 |
| LZ4 block archive | 190 | 2348 | 2208 | 3427 | 94 | 2244 | 16363 | 16923 | 1 | 4 |

Turbo/fade conclusion matches held-scroll: frame timing is effectively tied, but
the raw file path is faster inside the preview worker when the runtime cache is
warm. The archive path slightly increases placeholder frames in this single run.

### LZ4 archive scratch-buffer optimization

Follow-up optimization reused the archive loader's compressed and decompressed
scratch buffers and switched LZ4 block decode to `decompress_into`, avoiding two
fresh `Vec<u8>` allocations per screenshot. Same scenario: 20 second
`turbo-hold` with `MISTER_PREVIEW_TRANSITION=fade` and the LZ4-block archive.

| build | decoded previews | avg load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders |
|-------|------------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|
| before | 132 | 2743 | 99 | 2633 | 16351 | 17268 | 16 | 4 |
| after run 1 | 130 | 2733 | 97 | 2625 | 16372 | 17281 | 15 | 1 |
| after run 2 | 130 | 2598 | 108 | 2478 | 16356 | 17271 | 15 | 1 |

Result: keep the scratch-buffer change. The second after-run cut preview-worker
load time by about 145 us per decoded preview versus the fresh archive baseline,
and both after-runs reduced placeholder frames from 4 to 1. Frame wall timing
remained vsync-bound.

### Worker decoded-cache optimization

Follow-up optimization added a small worker-side decoded preview cache. This is
separate from the UI preview cache: it lets later selected/prefetch requests
reuse pixels already decoded by the preview worker, even if the UI did not apply
that result before the selection moved on. Same 20 second `turbo-hold` + fade +
LZ4-block archive benchmark.

| build | decoded previews | worker cache hits | avg load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders | RSS HWM KB |
|-------|------------------|-------------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|------------|
| scratch-buffer before | 130 | 0 | 2598 | 108 | 2478 | 16356 | 17271 | 15 | 1 | 29528 |
| decoded-cache run 1 | 132 | 4 | 2420 | 85 | 2314 | 16346 | 17289 | 15 | 4 | 36572 |
| decoded-cache run 2 | 130 | 2 | 2505 | 103 | 2384 | 16367 | 17234 | 16 | 1 | 36324 |

Result: keep the decoded-cache change as a CPU-for-memory trade. It cut
preview-worker load by about 93-178 us per decoded preview versus the best
scratch-buffer run, while adding about 6.8-7.0 MB RSS in this scenario. Frame
wall timing remained vsync-bound, and placeholder count was mixed across runs.

### Uncompressed indexed raw pack

Follow-up optimization added a `MMRAWP1\0` indexed packfile codec to the same
single-file archive path. This stores the original `.rgb565` payloads
uncompressed in one file, avoiding both exFAT many-file lookup and LZ4
decompression. The generated pack was 147,705,910 bytes, so this intentionally
misses the old 25 MB target.

Same 20 second `turbo-hold` + fade benchmark:

| build | archive bytes | decoded previews | worker cache hits | avg load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders | RSS HWM KB |
|-------|---------------|------------------|-------------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|------------|
| LZ4 decoded-cache run 1 | 19,351,762 | 132 | 4 | 2420 | 85 | 2314 | 16346 | 17289 | 15 | 4 | 36572 |
| LZ4 decoded-cache run 2 | 19,351,762 | 130 | 2 | 2505 | 103 | 2384 | 16367 | 17234 | 16 | 1 | 36324 |
| raw pack run 1 | 147,705,910 | 129 | 1 | 2086 | 629 | 1439 | 16352 | 17373 | 16 | 1 | 36148 |
| raw pack run 2 | 147,705,910 | 130 | 2 | 2274 | 779 | 1475 | 16358 | 17266 | 16 | 1 | 36352 |

Result: keep raw-pack support. It is not a storage optimization, but it was the
best real-app CPU result so far: about 231-419 us faster per decoded preview
than the LZ4 decoded-cache runs, and about 203-391 us faster than the fresh
separate raw-file turbo/fade baseline. Frame wall timing remained vsync-bound.

### Bulk raw565 decode copy

Follow-up optimization changed raw565 decode on little-endian targets from a
per-pixel `u16::from_le_bytes` push loop to one bulk byte copy into the final
`Vec<u16>`. Big-endian targets keep the old conversion loop. Same 20 second
`turbo-hold` + fade benchmark.

| path | build | decoded previews | worker cache hits | avg load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders |
|------|-------|------------------|-------------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|
| raw pack | before run 1 | 129 | 1 | 2086 | 629 | 1439 | 16352 | 17373 | 16 | 1 |
| raw pack | before run 2 | 130 | 2 | 2274 | 779 | 1475 | 16358 | 17266 | 16 | 1 |
| raw pack | after run 1 | 130 | 2 | 1545 | 741 | 778 | 16363 | 17253 | 16 | 1 |
| raw pack | after run 2 | 130 | 2 | 1477 | 673 | 784 | 16359 | 17261 | 15 | 1 |
| LZ4 block | after | 130 | 2 | 1916 | 93 | 1807 | 16351 | 17229 | 16 | 1 |

Result: keep the bulk decode change. On raw-pack it cut raw565 decode by about
660-690 us per decoded preview and total preview-worker load by about 609-797
us versus raw-pack before the change. The compressed LZ4-block path also improved
to 1916 us average load, making it faster than the original separate raw-file
turbo/fade baseline while keeping the small 19.35 MB archive.

### RAM-heavy archive preload

Follow-up optimization preloaded the selected archive into process memory when
the preview-loader thread starts. This is controlled by
`MISTER_PREVIEW_ARCHIVE_PRELOAD` and defaults on for archive runs; set it to `0`
to force the older per-entry file seek/read path. Same 20 second `turbo-hold` +
fade benchmark with raw pack.

| build | decoded previews | worker cache hits | avg load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders | RSS HWM KB |
|-------|------------------|-------------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|------------|
| before | 130 | 2 | 1527 | 760 | 744 | 16382 | 17224 | 16 | 1 | 36364 |
| preload lazy | 133 | 5 | 763 | 3 | 725 | 16357 | 17258 | 16 | 34 | 180420 |
| preload at worker start | 130 | 2 | 737 | 2 | 712 | 16379 | 17244 | 15 | 1 | 180428 |

Result: keep early archive preload. It roughly halved preview-worker load time
for raw-pack by removing per-preview `/media/fat` reads from the scroll path,
at a cost of about 144 MB additional RSS for the 147.7 MB raw pack. Lazy preload
was rejected because it caused 34 placeholder frames; preloading at worker start
restored placeholder behavior to baseline.

### Preview-loader priority tuning

Follow-up optimization changed the preview-loader thread from nice +10 to nice
+5. Nice 0 was also tested and rejected because it competed too aggressively
with the UI/apply path.

Same 20 second `turbo-hold` + fade benchmark with raw pack and archive preload:

| build | decoded previews | avg load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders | RSS HWM KB |
|-------|------------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|------------|
| nice +10 before | 130 | 737 | 2 | 712 | 16379 | 17244 | 15 | 1 | 180428 |
| nice 0 rejected | 131 | 705 | 2 | 680 | 16369 | 17299 | 13 | 180 | 180076 |
| nice +5 after | 130 | 589 | 2 | 562 | 16350 | 17233 | 16 | 1 | 180464 |

Result: keep nice +5. It cut preview-worker load by about 148 us per decoded
preview versus nice +10 without increasing placeholders or frame wall time. Nice
0 was rejected despite lower load time because placeholder frames jumped to 180.

### Automatic archive discovery

Follow-up optimization made archive use automatic for the default arcade preview
cache. Explicit `MISTER_PREVIEW_ARCHIVE` still wins. Without it, the loader looks
under the preview cache root for `raw565-<cache>-rawpack.mmraw` first and then
`raw565-<cache>-lz4block-12.mmlz4b`. Set `MISTER_PREVIEW_ARCHIVE_AUTO=0` to
force the old separate-file path.

Same 20 second `turbo-hold` + fade benchmark with no `MISTER_PREVIEW_ARCHIVE`:

| build | decoded previews | avg load us | avg read us | avg decode us | avg frame wall us | p95 frame wall us | slow >20ms | placeholders | RSS HWM KB |
|-------|------------------|-------------|-------------|---------------|-------------------|-------------------|------------|--------------|------------|
| before auto | 130 | 10215 | 9799 | 400 | 16341 | 17247 | 16 | 1 | 36020 |
| after auto | 130 | 667 | 2 | 646 | 16378 | 17246 | 15 | 1 | 180308 |

Result: keep automatic archive discovery. It moves the production/no-env path
from separate raw files to the RAM-backed raw pack when the pack exists, cutting
preview-worker load by about 9.5 ms per decoded preview in this run.

## Ten-screenshot drill-down

Follow-up command, run five times with page-cache drops before each raw read and
again before each archive read:

```bash
MISTER_BENCH_DROP_CACHES=1 \
  /media/fat/mister-magik/bench/preview-archive-bench compare-lz4-block \
  /media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320 \
  /media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320-lz4block-12.mmlz4b \
  1941u.rgb565 1944j.rgb565 3wondersr1.rgb565 asteroid.rgb565 btime.rgb565 \
  dkong.rgb565 galaga.rgb565 amidars.rgb565 arkanoid.rgb565 rtype2.rgb565
```

This measures load-to-ready-pixels: file/archive read, decompression for the
archive path, raw565 header parse, and materializing the full `Vec<u16>` pixel
buffer. It does not include the later framebuffer present/vsync.

Median of five cold runs:

| screenshot | raw total ms | LZ4-block total ms | compressed bytes |
|------------|--------------|--------------------|------------------|
| 1941u | 19.465 | 9.042 | 50,527 |
| 1944j | 9.513 | 6.938 | 72,038 |
| 3wondersr1 | 9.908 | 5.983 | 59,225 |
| asteroid | 11.542 | 3.318 | 3,608 |
| btime | 14.800 | 4.285 | 3,205 |
| dkong | 13.186 | 3.839 | 4,385 |
| galaga | 12.610 | 3.492 | 2,661 |
| amidars | 13.679 | 3.364 | 2,666 |
| arkanoid | 13.044 | 3.720 | 5,083 |
| rtype2 | 10.712 | 4.935 | 35,343 |

Charts:

- `build/arcade-screenshot-cache/hybrid-20260613/ten-screenshot-total-bars.png`
- `build/arcade-screenshot-cache/hybrid-20260613/ten-screenshot-lz4-phase-bars.png`
