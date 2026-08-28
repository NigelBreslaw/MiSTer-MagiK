# One-pass fast catalog optimisation

Date: 2026-08-28

The production fast catalog now shares one serial namespace inventory between
generic profile resolution, row construction, archive identities, directory
fingerprints, and refresh watches. Unknown runtime roots are inspected to depth
two and resumed only after their retained facts resolve to a launch profile.
No generic adapter contains card-specific rows or a snapshot of the tested SD
card.

OneLoad64 also emits its refresh observations during its prepared source pass.
The namespace inventory proves each accepted CRT is a regular file, avoiding a
second per-game metadata lookup while retaining the OneLoad installation
signature and excluded-tree checks.

## Real-hardware evidence

All values below are seconds. The retained catalog-build candidate is commit
`98e801318`; later descendants wire profiling and forced-refresh diagnostics
without changing catalog row or publication algorithms. The cold candidate was
measured immediately after an attended reboot. The original
126.12-second profile is attribution evidence rather than an unprofiled
like-for-like baseline.

| Phase | Original profile | Pre-C64 one-pass cold | Retained cold |
|---|---:|---:|---:|
| Separate profile/payload discovery | 46.38 | 0.22 | 0.22 |
| Complete source phase | 94.45 | 80.95 | 77.43 |
| Generic source work | 29.72* | 67.39 | 67.24 |
| Publication | 17.58 | 21.64 | 19.40 |
| Refresh snapshot capture | 7.78 | 3.93 | 1.34 |
| Complete catalog operation | 126.12 | 112.54 | 104.13 |
| Benchmark process completion | — | 117.45 | 110.86 |

`*` The old generic figure followed the separate 46.38-second profile scan and
therefore did not include the same cold filesystem work as the one-pass value.

The retained build published 90 systems and 33,728 games with logical
fingerprint
`30317c495d21ce86d78f0b8d7813f2a85c1ca98c47ca82ad4764803e860ae824`.
All three control samples had identical canonical rows and artifact identity.

C64 improved from 4.53 seconds of source work plus 2.59 seconds of duplicate
watch traversal to 0.79 seconds of source work plus 0.0004 seconds of watch
reuse. Complete snapshot capture fell below its 2.5-second target.

## Retained and rejected experiments

- Retained adaptive directory buffers: about 1.2 seconds less cold source time
  and about 0.8 seconds less warm source time.
- Retained shallow runtime discovery: unsupported roots stop at depth two;
  accepted roots resume from their retained frontier.
- Retained generic and OneLoad refresh-watch reuse: no second traversal when
  observations are complete.
- Rejected one-worker artifact staging pipeline: canonical output passed, but
  warm publication changed from roughly 19.9 seconds to a 21.5-second median.
  Copy hashing contended with SQLite's existing two-core search producer, so
  serial publication remains authoritative.
- Preallocation and `copy_file_range` remain rejected by earlier hardware
  experiments. The retained buffered publisher already combines copying and
  hashing in one streamed pass and performs one final filesystem sync.

## Remaining limit

The requested 90-second cold gate was not reached on this card. The retained
fresh operation is 104.13 seconds. The largest remaining cost is 67.24 seconds
of unavoidable cold generic source enumeration, led by collections containing
thousands of one-game directories. Skipping those directory opens would require
assuming a particular user's layout or trusting precomputed rows, both of which
would violate arbitrary-collection discovery and scratch-build parity.

The corrected forced-refresh benchmark also proved that the no-change target
had not previously been measured: the old benchmark's second leg performed a
strict registry load. Genuine unprofiled no-change refreshes took 2.67 and 3.04
seconds internally; a third sample rose to 18.77 seconds while the device
reported low-memory contention. All three opened zero row snapshots and wrote
zero SQLite, NavPack, or catalog artifacts. Reaching 0.5 seconds would require
not statting thousands of watched leaf directories, which would miss arbitrary
nested changes on exFAT unless a less conservative source-change contract is
accepted.

## Profiling evidence

The fast catalog worker now owns the `catalog-build-full` profiler lifecycle.
A forced refresh also bypasses deferred strict-load hydration, so fresh and
no-change profile legs both finalize. The successful cold fresh profile
captured 1,456 samples over 107.84 seconds: 866 resolved to folded stacks and
590 (40.5%) did not resolve to a symbol stack, down from the earlier 69.8%.

The ARM profiler records the interrupted PC before symbolication. Final
profiling additionally persists every unresolved PC and `/proc/self/maps`.
Verification produced a 34 KB raw-PC table and an 8.4 KB mapping file alongside
the folded stacks and flamegraph. In that warm verification, only 177 of 984
fresh samples (18.0%) lacked a resolved stack, and those samples remain
available for offline symbolication rather than being discarded.

The cold folded-stack attribution contained 1,456 hits. Inclusive categories
included 192 launcher-intro/UI hits, 94 SQLite/search hits, 84 namespace/syscall
hits, 67 SHA/checksum hits, and 47 artifact-copy hits. This profile is CPU
attribution only; the unprofiled control remains timing authority.
