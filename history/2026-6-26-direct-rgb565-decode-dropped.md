# Direct RGB565 Decode Experiment Dropped

Date: 2026-06-26

## Experiment

Tested item 1 from the screenshot decode/fewer-copy plan: decode v2 pixel
payloads directly into final RGB565 storage instead of using
`lz4 -> scratch Vec<u8> -> Vec<u16>`.

The branch changed the preview worker and `preview-pack-bench` diagnostic path so
the benchmark measured a direct final-buffer decode. The code was dropped after
the benchmark gate failed.

## Commands

Baseline diagnostics binary:

```bash
magik-gui/build-arm.sh --device --diagnostics --ui-scope launcher
scripts/mister agent deploy-magik-bin \
  magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb \
  /media/fat/mister-magik/mister-magik-fb
scripts/profile-preview-pack-decode.sh DIRECT565-BEFORE \
  --pack /media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b \
  --variant mmlz4b-v2-lz4-hc-9-pixels \
  --iterations 10 \
  --order random \
  --sample all
scripts/profile-first-preview.sh DIRECT565-FIRST-BEFORE --secs 8 --skip-build
```

After experiment binary:

```bash
magik-gui/build-arm.sh --device --diagnostics --ui-scope launcher
scripts/mister agent deploy-magik-bin \
  magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb \
  /media/fat/mister-magik/mister-magik-fb
scripts/profile-preview-pack-decode.sh DIRECT565-AFTER \
  --pack /media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b \
  --variant mmlz4b-v2-lz4-hc-9-pixels \
  --iterations 10 \
  --order random \
  --sample all
scripts/profile-first-preview.sh DIRECT565-FIRST-AFTER --secs 8 --skip-build
```

## Results

Pack decode:

```text
DIRECT565-BEFORE rows=8380 decode_cpu_p99_us=1662 raw565_parse_cpu_p99_us=1331 total_p99_us=2761
DIRECT565-AFTER  rows=8380 decode_cpu_p99_us=4048 raw565_parse_cpu_p99_us=25   total_p99_us=4108
```

The raw565 parse CPU p99 improved by 98.1%, but total p99 regressed by 48.8%.
The gate required at least 50% parse CPU p99 improvement and no more than 3%
total p99 regression, so the experiment failed.

First preview:

```text
DIRECT565-FIRST-BEFORE decoded_seen=1 apply_seen=1 decoded_load_source=index_pread apply_load_source=index_pread decoded_total_us=17019 apply_age_us=46348
DIRECT565-FIRST-AFTER  decoded_seen=1 apply_seen=1 decoded_load_source=index_pread apply_load_source=index_pread decoded_total_us=16906 apply_age_us=34466
```

The first-preview sample stayed valid and kept the same load source, but it did
not override the pack-decode gate failure.

## Decision

Drop the direct-final-buffer decode code. The experiment moved the copy/checksum
work out of the parse bucket, but the combined hot path was slower on the
MiSTer. Do not land this implementation as production code.

