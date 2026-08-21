# Arcade updater metadata local qualification

Date: 2026-08-21

This qualification used the exact Dev runtime from `928f73171`, the locally
verified v6 game-database candidate, and the pinned whole-card inputs. Later
source commits in the run change host-only delivery and benchmark tooling.

## Data and parity

- Updater index: 3,014 rows; 3,012 rows contain projection metadata.
- Compressed sidecar: 390,607 bytes, an increase of 102,554 bytes.
- Sidecar SHA-256: `ad175b9510c02d4bdead2fbd0018434843a297d7218d73f597afd15587152366`.
- Exact index hits: 2,990 of 3,004 installed MRAs; 14 local fallback reads.
- ROM inventory: 2,376 MAME filenames and 27 HBMAME filenames.
- Visible Arcade families: 922 before and after the metadata optimization.
- Typed visibility audit: zero primary-ROM false negatives.

## Authoritative unprofiled cold start

Evidence: `build/agent-benchmarks/cold-boot/1787344344`.

- Fresh-catalog mode was proven as `cold_no_catalog`; retained Arcade bootstrap
  data was not used.
- Arcade first-visible builder: 10.868 seconds, down from 20.039 seconds
  (9.171 seconds / 45.8%).
- First real launcher frame: 22.142 seconds, down from 26.591 seconds.
- MRA prefetch: 2.155 seconds, including 1.560 seconds of identity stats.
- ROM inventory: 0.446 seconds.
- Arcade discovery: 3.357 seconds; classification: 0.369 seconds.
- Projection preparation: 0.533 seconds.
- Bounded fallback metadata: 0.067 seconds, down from 10.328 seconds.
- Navigation snapshot: 0.019 seconds.

## CPU attribution

Evidence: `build/agent-benchmarks/cold-boot-pprof/1787345270`.

- 9,349 samples and 1,853 folded stacks were captured from a proven live scan.
- The profiled first-visible builder took 13.304 seconds and produced the same
  922 games.
- Startup-intro rendering accounted for 2,750 inclusive samples.
- The catalog builder accounted for 674 inclusive samples and the filesystem
  walker for 401, despite 6.863 seconds of profiled discovery wall time.
- This establishes that the remaining scan variance is primarily storage I/O
  and off-CPU latency rather than an unaddressed metadata CPU hotspot.

The fresh Arcade acceptance threshold is met locally: first-visible is below
15 seconds and the measured unprofiled run is below 20 seconds.
