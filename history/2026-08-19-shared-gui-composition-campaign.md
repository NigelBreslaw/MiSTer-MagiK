# Shared GUI composition campaign — 2026-08-19

This record covers the ownership repairs and follow-up optimizations made after
the shared HDMI/CRT composition plan. Development runs are directional
20-second `turbo` controls unless noted otherwise. They are not final
qualification; the final table is populated only by 40-second controls.

## Ownership and qualification hardening

The layer lifecycle now uses monotonic layout epochs, per-slot publication
identity, immutable transferred backings, role-indexed plans, and unioned
restore accounting. Preview failures are retryable, adopted backing storage is
owned by its publication, and orientation tests exercise both layer roles
through Normal → clockwise → counterclockwise → Normal transitions. The
benchmark now rejects pending cadence endpoints and non-authoritative terminal
captures instead of allowing those evidence gaps to pass.

These commits were validated by repository-owned host assurance because they
change state contracts, tests, profiling, or evidence handling rather than a
single hot rendering path:

- `ff91d6a1d` through `7935eff7a`: layout epochs, publication identity and
  backing ownership, role-indexed planning, restore accounting, preview
  recovery, and worker PMU attribution.
- `c813eec46` and `b761a35d5`: resolvable Streamline symbols and mutable
  rotation-surface repair.
- `70ecae708`: real two-slot, two-role orientation epoch sequences.
- `8c24d427a` and `f282c7c5c`: settled cadence and authoritative scanout become
  explicit qualification gates.

Artifact `1787136137` validates the completed gates on 720p landscape: 60.001
physical FPS, zero dropped/repeated refreshes, latch drops, sequence gaps,
ownership loss and record loss, settled endpoints, authoritative FPGA-latched
scanout, and 6.794 ms foreground P99.

## HDMI Arcade presentation

Portrait physical scrolls initially used hidden-slot read/shift/repair. Device
evidence showed that reading write-combined scanout memory cost more than a
dense write from retained RAM, so the accepted path keeps physical scroll
semantics in the producer but copies the dense publication without slot reads.

| Change | Artifact | Route | FPS | Foreground P95 / P99 | Result |
| --- | --- | --- | ---: | ---: | --- |
| Skip unproductive slot mirror work (`8c0d8e12e`) | `1787133030` | 720p portrait-left | 60.020 | 9.998 / 11.897 ms | Accepted; zero cadence/lifecycle errors |
| Landscape baseline | `1787133209` | 720p landscape | 59.967 | 6.099 / 6.862 ms | Reference |
| Sparse landscape diff (`f5d3562c0`) | `1787133733` | 720p landscape | 60.009 | 8.647 / 9.355 ms | Rejected; comparison cost dominated |
| Restore dense landscape copy (`826720908`) | `1787133993` | 720p landscape | 60.032 | 6.092 / 6.684 ms | Accepted; baseline performance restored |

The sparse experiment produced only 52 sparse frames against 1,182 full-copy
frames because alternating rows change most pixels. Its unreachable scratch,
diff, and test scaffolding was removed by `fa5aba5d9`.

## CRT retained overlay

Holding the previous settled backdrop removed the plain-frame flash while a
replacement was still being prepared. Full backdrop-copy incidence fell from
1,151/2,467 frames (46.7%) in `1787096035` to 339/1,264 (26.8%) in
`1787134324`, and copy P95 fell from 3.710 to 1.879 ms.

| Change | Artifact | Route | FPS | Overlay P95 / P99 | Foreground P95 / P99 | Result |
| --- | --- | --- | ---: | ---: | ---: | --- |
| Hold prior backdrop (`dcd6f26dc`) | `1787134324` | CRT 240p portrait-left | 60.070 | 8.065 / 8.423 ms | 11.738 / 12.261 ms | Accepted |
| Sort physical spans (`6aa7b3520`) | `1787134631` | CRT 240p portrait-left | 60.053 | 8.561 / 9.082 ms | 12.105 / 12.860 ms | Rejected |
| Union logical row damage (`73edf5bdb`) | `1787134899` | CRT 240p portrait-left | 60.035 | 6.790 / 7.086 ms | 10.531 / 11.018 ms | Accepted |
| Resolve pixels by row (`532fb511f`) | `1787135146` | CRT 240p portrait-left | 60.058 | 4.238 / 4.443 ms | 8.123 / 8.580 ms | Accepted |
| Validate 288p retained overlay | `1787135227` | CRT 288p portrait-left | 50.442 | 4.181 / 4.501 ms | 8.528 / 9.722 ms | Accepted |

The final CRT row implementation resolves ring and selection state once per
logical row and walks monotonic text runs. It preserves complete CRT
cached-frame ownership; it does not adopt HDMI direct-layer semantics.

## Final qualification

Only portrait-left is device-qualified. The opposite rotation remains covered
by host pixel and ownership parity. HDMI is qualified at both output modes.

| Route | Artifact | Physical FPS | Foreground P99 | Repeats / drops / latch / gaps / ownership / record loss | Terminal | Result |
| --- | --- | ---: | ---: | --- | --- | --- |
| 720p landscape | `1787136283` | 59.995 | 7.687 ms | 0 / 0 / 0 / 0 / 0 / 0 | FPGA-latched | PASS |
| 720p portrait-left | `1787136368` | 60.005 | 12.321 ms | 0 / 0 / 0 / 0 / 0 / 0 | FPGA-latched | PASS |
| 1080p landscape | `1787136454` | 60.004 | 6.606 ms | 0 / 0 / 0 / 0 / 0 / 0 | FPGA-latched | PASS |
| 1080p portrait-left | `1787136541` | 59.997 | 9.440 ms | 0 / 0 / 0 / 0 / 0 / 0 | FPGA-latched | PASS |
| CRT 240p portrait-left | `1787136630` | 60.055 | 9.473 ms | 0 / 0 / 0 / 0 / 0 / 0 | FPGA-latched | PASS |
| CRT 288p portrait-left | `1787136721` | 50.429 | 10.159 ms | 0 / 0 / 0 / 0 / 0 / 0 | FPGA-latched | PASS |

All six artifacts have `evidence_authority=qualification`, a requested and
measured 40-second hold, settled presentation endpoints, and exact route
restoration. The runner restored the original 720p HDMI mode plus the original
`MiSTer.ini`, settings, launcher environment, installed manifest, boot identity,
and launcher health after every leg.
