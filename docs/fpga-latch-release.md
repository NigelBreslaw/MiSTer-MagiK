# FPGA vblank latch release requirements

This document defines the commercial-release contract for the small Menu FPGA
delta used by MiSTer MagiK. Its scope is only the `0x57` set command, the `0x58`
status command, and the vblank-latched framebuffer route. The retired scanout
mailbox, commands `0x59`/`0x5a`, AXI/ACP access, DMA ownership, and completion
fences are not part of this design or its qualification.

## Behavioral requirements

- **LATCH-001 — command discovery.** Starting command `0x57` returns `0x4d47`;
  starting command `0x58` returns `0x4d48`. Other commands are unaffected.
- **LATCH-002 — route staging.** `0x57` words 0–9 stage enable/filter/format,
  base, width, height, horizontal and vertical bounds, and stride. Staging these
  words must not change the active framebuffer route or create a pending post.
- **LATCH-003 — post commit.** Word 10 supplies the 16-bit sequence, marks the
  staged route pending, and increments the 16-bit post counter.
- **LATCH-004 — vblank application.** A pending route is applied atomically on
  the next rising edge from the synchronized `hdmi_vbl` signal, and never on a
  falling edge or outside vblank. Application copies every staged route field,
  makes the pending sequence active, increments the 16-bit flip counter, and
  clears pending.
- **LATCH-005 — pending replacement.** If word 10 commits while a request is
  already pending, the newly staged request replaces it, the post counter still
  increments, and the 16-bit drop counter increments once. The next eligible
  vblank applies the replacement request.
- **LATCH-006 — status layout.** `0x58` words 0–10 return, respectively: active
  sequence; pending sequence; `{13'b0, pending, pending_enable, active_enable}`;
  flip count; post count; drop count; active base low; active base high; active
  width; active height; and active stride.
- **LATCH-007 — initialization.** All MagiK staging, sequence, counter, pending,
  and vblank-synchronizer registers power up to zero. The delta has no runtime
  reset interface and must not add one implicitly.
- **LATCH-008 — finite-width behavior.** Sequences are opaque 16-bit values.
  Post, flip, and drop counters wrap modulo 65,536; wrapping must not alter
  pending or route behavior.
- **LATCH-009 — compatibility.** Stock `UIO_SET_FBUF` behavior and all unrelated
  Menu commands remain unchanged. The latch remains compatible with the
  hidden-slot renderer and its `/dev/fb0` fallback.

## Requirement-to-test matrix

These are release gates. A row is not satisfied merely because a test is named;
the retained result must identify the exact source and RBF hashes under test.

| Requirement | RTL simulation / assertion | Integration or hardware evidence |
| --- | --- | --- |
| LATCH-001 | Both magic values; unrelated opcode has no latch response | Passive `fpga-latch-report` reports both commands supported |
| LATCH-002 | Exercise every word and prove no early pending/apply | Posted geometry and active route match |
| LATCH-003 | Sequence commit, pending, and post-count checks | Post counter advances during launcher motion |
| LATCH-004 | Rising/falling/no-edge cases; atomic-route assertion | Flip counter advances with zero visual/alternation misses |
| LATCH-005 | Two posts before vblank; replacement and one drop | Deliberate over-post increments drop once, then recovers |
| LATCH-006 | Exact readback for all eleven status words | Passive report agrees with active route and counters |
| LATCH-007 | Power-up state and bounded startup checks | Cold boot and RBF reload start safely |
| LATCH-008 | Sequence and all counter wrap cases | Long soak shows continued counter progress |
| LATCH-009 | Patch integration, opcode collision check, stock comparison | Lifecycle tests and clean `/dev/fb0` fallback |

## Custom-delta release signoff

A release candidate is eligible only when all of the following are retained:

1. The exact MagiK commit, pinned upstream Menu commit, latch patch hash,
   Quartus 17.0 build identity and seed, RBF SHA-256, and report hashes.
2. Passing self-checking RTL tests for every requirement above, complete
   reachable line/branch and functional-point coverage, and reviewed assertions
   for vblank-only and atomic application behavior.
3. A stock-versus-patched Quartus comparison made with identical inputs. The
   patch may add no warning, unconstrained endpoint, inferred latch, negative
   slack, non-zero TNS, or unreviewed CDC finding. The two-stage vblank
   synchronizer must be recognized and have a documented MTBF disposition.
4. Hardware qualification tied to the candidate RBF hash: both command magic
   values, advancing post/flip counters, zero unexpected drops, supported video
   geometries, lifecycle and fallback cases, deliberate-overflow recovery, a
   two-hour soak, and physical HDMI inspection. Qualification on only one MiSTer
   remains single-unit qualification; commercial signoff requires a second
   representative MiSTer/display combination.
5. Independent review of the custom RTL, Menu integration, constraints, waiver
   ledger, and retained evidence, plus a tested stock-RBF rollback.

## Artifact status

The latch-only RBF from GitHub Actions run `29153173239`, SHA-256
`7f4f5c40260f52341f11f3cc66891c551699376dd89fc39ff03efdebd48eb5c2`, is the
behavioral baseline documented in
[the zero-copy retirement record](../history/2026-07-11-zero-copy-retirement.md).
It is not the final release artifact; extraction and delta signoff will produce
a new candidate hash.

The current ignored local RBF has SHA-256
`f61ad600ad63d8e91fc9fa8b093448fcecdd5d4370b5d330456bc53f19d5a17c` and its
metadata includes the retired mailbox patch and RTL. It is stale, is not the
latch-only baseline, and is ineligible for release qualification or deployment.

The extracted-latch candidate and its current qualification disposition are
recorded in [FPGA latch release signoff](fpga-latch-release-signoff.md). A
candidate hash listed there is not an approved commercial release while any
custom-delta checklist item remains open.
