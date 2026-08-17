# Thin-LTO 32-codegen-unit release-device campaign result

Thin LTO with 32 codegen units was measured against the thin-LTO/16-CGU
baseline median of `35.856 s`. The build retained `opt-level=3`, ARM tuning,
`panic=abort`, `strip=debuginfo`, and the `ui,profile` features.

| sample | seconds |
| --- | ---: |
| cold | 145.997 |
| no-op 1 | 3.602 |
| no-op 2 | 3.243 |
| no-op 3 | 3.238 |
| no-op 4 | 3.274 |
| no-op 5 | 3.235 |
| edit warmup | 42.230 |
| edit 1 | 32.764 |
| edit 2 | 35.350 |
| edit 3 | 31.225 |
| edit 4 | 30.246 |
| edit 5 | 29.757 |
| edit median | **31.225** |

All five edit samples beat the thin-LTO/16-CGU baseline. The typed campaign
advanced the sequential baseline to
`/private/tmp/mister-magik-campaign/baseline-thin-cgu32.json`; the next logical
commit promotes this profile for ordinary `release-device` delivery.

Binary size is not a gate. The retained function symbols keep dormant on-device
pprof available for the existing profiling contract.
