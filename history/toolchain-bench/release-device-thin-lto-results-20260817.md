# Thin-LTO release-device campaign result

Thin LTO was measured at revision `6ceaa87fa` against the production fat-LTO/2
CGU baseline median of `91.672 s`. The build remained `opt-level=3`, ARM tuned,
`panic=abort`, `strip=debuginfo`, and retained the `ui,profile` features; only
the LTO strategy and codegen-unit count changed.

| sample | seconds |
| --- | ---: |
| cold | 149.868 |
| no-op 1 | 4.474 |
| no-op 2 | 3.461 |
| no-op 3 | 3.284 |
| no-op 4 | 3.289 |
| no-op 5 | 3.247 |
| edit warmup | 49.483 |
| edit 1 | 35.856 |
| edit 2 | 36.645 |
| edit 3 | 35.849 |
| edit 4 | 36.131 |
| edit 5 | 34.791 |
| edit median | **35.856** |

All five edit samples were below the baseline median, so the typed campaign
advanced the baseline to `/private/tmp/mister-magik-campaign/baseline-thin.json`.
Binary size is intentionally not a gate for this campaign. The profile retains
function symbols and remains subject to device performance, memory, and frame
cadence sign-off.

For reference, the same external target directories produced 24,479,424 bytes
for fat-LTO/2 and 30,955,412 bytes for thin-LTO. That size increase does not
change the compile-time decision.
