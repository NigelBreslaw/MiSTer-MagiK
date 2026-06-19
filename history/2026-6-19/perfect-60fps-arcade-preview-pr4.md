# Perfect 60fps Arcade Preview PR4 Evidence

Date: 2026-06-19

Purpose: lock the final release gate so preview fade pacing regressions fail
with a compact command.

## Change

- Added `scripts/gate-preview-60fps.sh`.
- The wrapper runs 60s held-scroll fade and 60s turbo-hold fade.
- It fails if either trace has:
  - true work misses after frame 30,
  - vsync fallback/timeout/error or any other non-vsync source,
  - non-zero max vsync miss streak,
  - p99 work at/above the configured threshold.
- Added a parser self-test with synthetic good, work-bad, p99-bad, and
  vsync-bad traces.
- Updated `docs/benchmarking.md` with the final gate command.

## Self-Test

```bash
scripts/gate-preview-60fps.sh --self-test
```

Result:

```text
gate-preview-60fps self-test ok
```

## Final Device Gate

Command:

```bash
scripts/gate-preview-60fps.sh PR4-AFTER2-GATE --skip-build --visual-captures 0
```

Result:

```text
PR4-AFTER2-GATE preview 60fps gate passed
```

| Label | Frames after 30 | p99 work us | Work >16.7ms | Vsync/fallback/timeout/error | Max miss streak |
| --- | ---: | ---: | ---: | --- | ---: |
| PR4-AFTER2-GATE-FADE-VEL | 3567 | 13614 | 0 | 3567/0/0/0 | 0 |
| PR4-AFTER2-GATE-FADE-TURBO | 3565 | 13664 | 0 | 3565/0/0/0 | 0 |

An earlier `PR4-AFTER-GATE` run failed turbo with one true work miss at frame
35 while heavy startup prefetch decode was active. The rerun above is the final
passing gate evidence; the wrapper correctly failed the contaminated run.
