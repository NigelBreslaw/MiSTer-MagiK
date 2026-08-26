# Fast five-system cold comparison

Date: 2026-08-26

Branch: `nigel/arcade-catalog-prototype`

## Result

The parallel fast-five catalog can drive the real Dev UI with exactly Arcade,
Amiga, C64, DOS, and X68000. The candidate contained 18,078 games and matched
the five-system production reference with zero missing, unexpected, or changed
rows. Startup loaded five registry systems from the separate root with refresh
disabled and the expected artifact fingerprint.

Every retained timing below is a fresh isolated generation after a verified
supervised reboot. The old builder used one separate reboot and root per
system. The new publisher used one reboot, then materialized the five immutable
system artifacts sequentially from precomputed final rows. Warm and
forced-rebuild runs are excluded.

| System | Games | Old builder | New cache publication | Speedup | Saved |
| --- | ---: | ---: | ---: | ---: | ---: |
| Arcade | 922 | 48.370 s | 1.253 s | 38.59x | 47.116 s |
| Amiga | 1,561 | 8.180 s | 1.624 s | 5.04x | 6.556 s |
| C64 | 15,089 | 31.346 s | 21.945 s | 1.43x | 9.401 s |
| DOS | 237 | 9.999 s | 0.352 s | 28.43x | 9.647 s |
| X68000 | 269 | 9.033 s | 0.370 s | 24.40x | 8.663 s |

The per-system new figures are the measured artifact-publication components.
The complete new command read, decoded, and validated the 13,734.6 KB frozen
snapshot in 1.443 seconds, then published all artifacts and the manifest in
26.078 seconds. Its full cold elapsed time was 27.527 seconds, compared with
106.927 seconds of summed old authoritative-builder work: 3.88x faster and
79.400 seconds less work.

## Meaning and limitation

This proves the parallel format, artifact publisher, real-UI selector, exact
row comparator, and the upper-level value of ahead-of-time catalog rows. It is
not yet a complete end-to-end timing for every new discovery adapter. The
measured snapshot was generated before the reboot from the five-system
reference. Release-receipt validation and custom-content fallback must be
included when the AmigaVision, OneLoad64, and Neon68K adapters are moved into
the independent builder. The checked-in 0MHz and independent Arcade work have
separate cold evidence, but are not yet composed into this exact-row snapshot.

The prototype intentionally reuses the current SQLite/navigation/NavPack
artifact writer to exercise the real UI. This exposes the next major legacy
cost: C64 artifact creation alone takes 22.645 seconds for 15,089 rows. Device
logs attribute much of that to SQLite FTS construction, optimization,
integrity checking, and exFAT writes. The new architecture should publish a
NavPack plus a compact purpose-built search sidecar and stop building SQLite
and compressed duplicate navigation. That is expected to remove most of the
remaining 27.527 seconds, but it requires a new measured result before any
specific saving is claimed.

## Evidence

- New cold publication and UI parity:
  `build/agent-benchmarks/fast-five-ui-prototype/6345ef164/cold-report.json`
- Final old per-system cold matrix:
  `build/agent-benchmarks/fast-five-old-cold/acd396dfb/report.json`

Production registries were unchanged. The old benchmark roots were removed,
and the normal launcher was restored after the matrix. No branch was pushed.
