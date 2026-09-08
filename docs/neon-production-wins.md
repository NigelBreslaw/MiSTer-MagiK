# Measured Cortex-A9 production optimisations

This integration includes only the four selected numerical implementations and
their shared ownership/parity tests. It does not restore retired tooling, add
runtime tuning switches, or include rejected experiment implementations.

| Change | Fixed-work Mini baseline / candidate | Full-app evidence |
| --- | --- | --- |
| E01 signed high-multiply RGB565 blend | 2186 / 1445 ms, about 34% faster | Linked production dispatch inspected; selection journey did not establish an overall app speedup |
| E02 multiplication-free half blend | 336.5 / 224.3 ms; matched-core PMU supports about 25% | Exact output; do not use cross-core timing alone to claim 33% |
| E13 shared neighbourhood work across five screenshot phases | 71.6 / 64.0 ms, about 11% faster | Two live screensaver windows passed 598 animation intervals each with zero drops/rejections |
| E11 retained orientation rectangle differences | 540.5 / 220.9 ms, about 59% faster | 168.5 / 72.5-73.3 ms zoom work over 190 frames; endpoint captures byte-identical |

Times are workload/stage totals, not per-frame latency or additive whole-app gains.
RGB565 rounding, saturation, opacity endpoints, coverage and retained-buffer
semantics are preserved. The original 128-byte scanout-copy prefetch is unchanged:
removing it was about 38% slower and tested alternatives did not establish a win.

## Evidence and remaining qualification

The campaign history and ignored raw artifacts remain on the local
`nigel/neon-production-wins` branch at `f85b3711c`, including the experiment index
at its historical `magik2/docs/neon-campaign.md` path. Raw binaries, screenshots
and profiles are not part of this PR. Rejected/inconclusive E03-E10, E12, E14 and
E17-E19 remain on their original experiment branches, not in this source tree.
E15/E16 were not implemented. Do not merge rejected candidates to retain history.

Full-app baseline run `20260908T005513Z-8492dc6fbc05` uses ELF
`4170686d9ddc40e9c73e3b46dd31929b4a1672d9f1f2821e5810f0e5ed6fe09a`.
Candidate runs `20260908T010019Z-32528ee3bd07` and
`20260908T183018Z-601fc5bfadc9` use ELF
`1c223e5383fa8e6cc3817360dd9920953d1751df61daa80cd53e083121093289`.
Their portrait PNGs are byte-identical, SHA-256
`57646dea9659ea93e6d1d0b82856faaafeefb56ab250bb2968922272d8c5ff41`;
restored PNGs are also byte-identical,
`712e496fdf8136fe5cda313278ccb3fc840cb546ee544ec86340ba57e27c0e92`.
The earlier conversational E11 text-loss attribution was incorrect.

Both baseline and candidate orientation journeys retain one physical dropped
frame, with zero latch rejections/invalid ownership intervals. Startup retains
two drops. These are open zero-drop qualification failures, not waived by the
performance wins. Endpoint parity is not an all-frame correctness proof.

E13 candidate run `20260908T010119Z-39ad833b7987` prepares 53/52 live cards
with 2516044/2544959 us cumulative phase work, versus baseline
`20260908T005649Z-45b19560cd3e`: 52 cards and 2945311 us. Live geometry and
scheduling differ, so these numbers support rather than replace fixed-fixture
Mini evidence. Candidate windows have zero physical drops/latch rejections;
one native screenshot was available and inspected.

Exact linked-ELF disassembly verifies both affected kernels and production
callers. Stack entry saves, invariant reloads, arrays and confirmed spills are
classified separately. PMU cache-dependent stalls are not all data dependencies,
L1 refills are not DRAM misses, and NEON clock-enabled cycles are not utilisation.

This PR ports the production changes onto the current tooling-retirement base.
The historical ELF hashes above describe campaign measurements, not the new
integration binary. Current integration checks are reported separately in the PR.
