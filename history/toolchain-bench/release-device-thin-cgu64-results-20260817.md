# Thin-LTO 64-codegen-unit release-device campaign result

Thin LTO with 64 codegen units was measured against the promoted thin-LTO/32-CGU
baseline median of `31.225 s`. The build retained `opt-level=3`, ARM tuning,
`panic=abort`, `strip=debuginfo`, and the `ui,profile` features.

| sample | seconds |
| --- | ---: |
| cold | 126.613 |
| no-op 1 | 3.149 |
| no-op 2 | 3.187 |
| no-op 3 | 2.948 |
| no-op 4 | 2.971 |
| no-op 5 | 2.844 |
| edit warmup | 37.020 |
| edit 1 | 29.356 |
| edit 2 | 29.703 |
| edit 3 | 35.280 |
| edit 4 | 33.906 |
| edit 5 | 34.119 |
| edit median | **33.906** |

Only two of five edit samples beat the 32-CGU baseline, so the typed campaign
rejected this candidate. The candidate implementation is reverted in the next
commit; 32 codegen units remains the canonical release-device setting.
