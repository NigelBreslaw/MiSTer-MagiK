# Presentation-ready 320x320 preview pack experiment dropped

Date: 2026-6-26

## Item

Tested an experimental `mmlz4b-v3-boxed-320x320` pack format where each entry
stores a full 320x320 RGB565 preview rectangle. The intended runtime win was to
avoid per-frame preview scaling and margin clearing so preview draw could use a
straight row copy.

## Commands

Before pack decode:

```bash
scripts/profile-preview-pack-decode.sh BOXED320-BEFORE \
  --pack /media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b \
  --variant mmlz4b-v2-lz4-hc-9-pixels \
  --iterations 10 --order random --sample all
```

Before scroll:

```bash
scripts/profile-preview-scroll.sh 60 turbo-hold BOXED320-BEFORE-TURBO \
  --skip-build --visual-captures 0
```

Experiment pack recode:

```bash
scripts/magik-cloud run -- cargo run --quiet -- pack-recode \
  --input packs/arcade-screenshots.mmlz4b \
  --output /private/tmp/arcade-screenshots-boxed320.mmlz4b \
  --variant mmlz4b-v3-boxed-320x320
scripts/mister put /private/tmp/arcade-screenshots-boxed320.mmlz4b \
  /media/fat/mister-magik/bench/arcade-screenshots-boxed320.mmlz4b
```

After pack decode:

```bash
scripts/profile-preview-pack-decode.sh BOXED320-AFTER \
  --pack /media/fat/mister-magik/bench/arcade-screenshots-boxed320.mmlz4b \
  --variant mmlz4b-v3-boxed-320x320 \
  --iterations 10 --order random --sample all
```

After scroll:

```bash
MISTER_PREVIEW_ARCHIVE=/media/fat/mister-magik/bench/arcade-screenshots-boxed320.mmlz4b \
  scripts/profile-preview-scroll.sh 60 turbo-hold BOXED320-AFTER-TURBO \
  --skip-build --visual-captures 0
```

## Results

Pack bytes:

- Before arcade pack: 24,529,459 bytes.
- After arcade pack: 24,786,708 bytes.
- Growth: 1.05%, within the 15% limit.

Pack decode:

- Before `total_p99_us`: 2817.
- After `total_p99_us`: 3045.
- Result: 8.1% regression, failing the "no more than 5%" gate.
- Before `raw565_parse_cpu_p99_us`: 1324.
- After `raw565_parse_cpu_p99_us`: 1416.

Turbo scroll:

- Before `preview_blit_us` p99: 1635.
- After `preview_blit_us` p99: 1631.
- Result: 0.2% improvement, failing the required 25% improvement.
- Vsync fallback/timeout/error counts stayed zero in both runs.

## Decision

Dropped. The boxed pack slightly increased pack size but also increased decode
tail latency and did not materially improve runtime preview blit cost. All code
and private submodule changes from the experiment were removed; only this note
was kept.
