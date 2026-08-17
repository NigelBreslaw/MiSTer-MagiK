# Thin-LTO plus symbol stripping campaign result

This candidate was measured against the thin-LTO baseline median of `35.856 s`.

| sample | seconds |
| --- | ---: |
| cold | 246.563 |
| no-op 1 | 3.846 |
| no-op 2 | 3.207 |
| no-op 3 | 3.328 |
| no-op 4 | 3.205 |
| no-op 5 | 3.266 |
| edit warmup | 42.690 |
| edit 1 | 33.468 |
| edit 2 | 32.741 |
| edit 3 | 32.306 |
| edit 4 | 32.625 |
| edit 5 | 32.299 |
| edit median | **32.625** |

All five edit samples beat the thin-LTO baseline. The candidate artifact is
fully stripped, so `nm` finds no retained launcher or pprof symbols. It remains
a provisional compile-time winner for the later profiler-removal experiment;
the regular thin-LTO profile remains the active parity baseline while dormant
on-device profiling is part of the production contract.
