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
