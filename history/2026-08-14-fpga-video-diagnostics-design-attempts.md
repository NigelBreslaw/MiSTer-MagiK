# FPGA video diagnostics: two attempted designs and their retirement

Date: 2026-08-14

This document records the two FPGA diagnostic designs developed to investigate
the rare Arcade-to-MiSTer-MagiK return failures:

- a black screen with live software and framebuffer content; and
- a separate vertical-colour-band failure.

It records the aims, architecture, incremental corrections, measured results,
and the evidence-led reasons neither design is a releasable final solution.
It is history, not current implementation policy.

The two designs are:

1. the original wide, three-domain first-fault snapshot observer; and
2. the replacement staged observer, which began with the small `0x60` lock
   recorder, expanded through final/scaler/Avalon evidence, and ultimately
   incorporated a lossless scaler-completion repair.

## Common constraints and acceptance policy

Both designs were required to be passive unless a separately reviewed
functional repair was explicitly introduced. Diagnostic logic was not allowed
to drive the framebuffer route, video mux, PLL, reset, HDMI pins, or Avalon
control path.

The important fixed qualification rules were:

- Quartus Prime Lite 17.0.0 Build 595;
- canonical fitter seed 2;
- matched stock, pinned pre-observer, and patched variants;
- zero total negative slack;
- setup and hold above the active fixed thresholds;
- final unconstrained-output count equal to the pinned baseline, normally 158;
- exact, reviewed CDC crossings and timing constraints;
- no timing waiver, seed sweep, LogicLock placement patch, legacy-scaler timing
  edit, or relaxed acceptance threshold to make an experiment pass; and
- no RBF installation when its canonical signoff was invalid.

The absolute setup floor was initially 0.200 ns. Later qualification also
enforced no more than 0.150 ns degradation from the matched baseline, which
made the effective setup requirement 0.428 ns for the final experiments.

The local/GitHub build contract was corrected during the work so that build
date, four-processor policy, preparation script, seed, and synthesis inputs
were part of the implementation identity. Earlier local comparisons that mixed
build dates were not used to waive later failures. See
[FPGA signoff build-identity correction](2026-08-13-fpga-signoff-build-identity.md).

---

## Design 1: wide three-domain first-fault snapshot observer

### Aim

The first design attempted to capture a coherent, detailed incident across all
three relevant FPGA clock owners:

- `clk_sys` control and route state;
- Avalon/framebuffer request and return state; and
- final HDMI/output state.

The intended record included the physical HDMI FPLL status, route generation,
framebuffer base and geometry, control sequence, reference timing, fault
classification, output payload, and manual or automatic capture context. The
goal was to preserve the first fault even if one native clock later stopped.

### Architecture

The design used separate control, Avalon, and output observers with:

- wide held payloads and bundled-data CDC;
- generation and route-context buses crossing clock domains;
- request, pending, fault, trigger, freeze, acknowledgement, and manual-capture
  arbitration;
- first-fault priority and frozen snapshots; and
- commands `0x5d` through `0x5f` for the multi-record readout.

In fitted form it cost roughly 930-956 ALMs and 1,433-1,467 registers. The size
was above the original target of keeping diagnostics under approximately 800
ALMs, and the widespread registers and fanout made unrelated legacy placement
sensitive.

### Incremental fixes

#### 1. Original seed-2 observer — `7bb59b65`

The original design completed simulation and CDC assurance but failed canonical
local timing:

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.075 ns |
| Hold slack | 0.231 ns |
| TNS | 0 |
| Unconstrained outputs | 158 / 158 |
| Resource delta | +930 ALMs / +1,433 registers |

Two observer paths were themselves too close to the floor: 0.153 ns and
0.188 ns from `reference_lines[5]` to output fault flags. This justified one
targeted observer-path correction rather than a seed or constraint change.

#### 2. Control cleanup — `980e6905`

The control observer was simplified to remove redundant logic and reduce the
large shared control hierarchy. Fitted control usage fell by about 20.5 ALMs
and nine registers in the measured hierarchy.

This was a genuine local reduction, but it did not materially reduce the
whole-design observer footprint because fitting, packing, and duplication
changed elsewhere.

#### 3. Pipelined final fault capture — `6b4d2e9d`

A selected-fault pipeline was inserted in the output observer to remove the
two direct low-margin diagnostic paths.

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.052 ns |
| Hold slack | 0.242 ns |
| TNS | 0 |
| Unconstrained outputs | 158 / 158 |
| Resource delta | +937 ALMs / +1,467 registers |

The targeted paths disappeared. The worst remaining diagnostic path improved
to 0.543 ns, from `frame_period` to `snapshot_generation`. The global worst
path, however, moved to unrelated legacy `ascal` logic and setup became worse.

This established an important pattern repeated throughout the project: fixing
the diagnostic critical cone did not make whole-device placement monotonic.

#### 4. Capture generation at request recognition — `b43ef0ea`

The final bounded attempt moved `snapshot_generation` capture to request
recognition. It removed generation from the later arbitration mux and was
intended to eliminate the remaining repeated generation-enable paths without
adding another register stage.

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.285 ns |
| Hold slack | -0.293 ns |
| TNS magnitude | 0.293 ns |
| Unconstrained outputs | 158 / 160 |
| Resource delta | +956 ALMs / +1,445 registers |

This exposed a real architectural defect rather than another harmless placement
move. The failing hold path was the diagnostic bundled-data crossing:

```text
control|expected_route_epoch[12] -> output|route_epoch[12]
```

It crossed from `clk_sys` to the HDMI domain with zero logic levels. The fitter
also created two new unconstrained LED duplicate rows.

### Why Design 1 failed

Design 1 failed for three independent reasons:

1. **It was too large.** Its steady fitted cost remained approximately
   930-956 ALMs and 1,433-1,467 registers despite local cleanup.
2. **It was physically invasive.** Wide payloads, duplicated context, and
   high-fanout freeze/arbitration state changed global packing and placement.
   Unrelated scaler critical paths moved substantially between otherwise small
   observer edits.
3. **Its CDC boundary was unsound.** The final experiment produced an actual
   multibit bundled-data hold violation and changed the unconstrained-path
   topology.

The important lesson was not merely that a particular RTL expression was slow.
The three-domain coherent snapshot abstraction itself was the wrong boundary.
Further pipeline stages, classifier stages, constraints, or seed changes would
have treated symptoms while preserving the architectural cause.

All Design 1 RBFs were rejected and not installed. The detailed retained
measurements are in
[Seed-2 FPGA video diagnostics retirement evidence](2026-08-12-fpga-seed-2-video-diagnostics-retirement.md).

---

## Design 2: staged narrow observers plus scaler-completion repair

### Aim

The replacement deliberately abandoned coherent cross-domain incident
snapshots. It attempted to answer one boundary question at a time with:

- local-domain state only;
- single-bit event toggles through independent two-register synchronizers;
- short, read-only, atomically snapshotted UIO records;
- no pixel, address, route, or payload bus crossing clock domains; and
- evidence-triggered expansion rather than implementing every possible probe
  up front.

This design initially targeted diagnosis only. Live field evidence later
identified a concrete lossy CDC mechanism inside `ascal`; the design then grew
to include a functional completion-credit repair and narrow repair-health
evidence.

### Stage A: physical FPLL lock recorder (`0x60`)

#### Implementation — `6403cc81`, `840605cf`, `23b5f5d2`

The first milestone replaced the entire wide observer with one small
`clk_sys`-domain recorder:

- physical lock source `reconfig_from_pll[16]`;
- one exact two-register synchronizer;
- sticky seen-high, armed, current-lock, ever-lost, saturated loss count, and
  overflow state; and
- a four-word, read-only `hdmi-lock-evidence-v1` record on `0x60`.

The fitted result passed its then-current gates:

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.474 ns |
| Hold slack | 0.247 ns |
| TNS | 0 |
| Unconstrained outputs | 158 / 158 |
| Resource delta | +64 ALMs / +25 registers |

This milestone was successful and useful. It was deployed through the typed
transaction and captured a black-screen occurrence while remaining readable.
It proved that the real physical FPLL was armed, currently locked, and had not
added a new loss during that occurrence.

The first apparent live `0x60 = 0/0` failure was not accepted as an RTL timing
conclusion. Delivery activation and UIO command-boundary assurance were fixed:

- experimental activation was tied to the manifest-selected Dev RBF rather
  than the stock-owned `/media/fat/menu.rbf` alias;
- replacement Main generation/PID and launcher ownership were checked; and
- agent v22 added an acknowledged IO-enable-low strobe boundary so a new
  command could not be interpreted as data for the previous command.

These were transport and identity corrections, not FPGA logic fixes.

### Stage B: final registered HDMI activity (`0x61`)

#### Aim

Because lock remained high during a black screen, the next observer sampled
only the final pin-driving HDMI registers. Each completed frame was classified
as:

- no DE;
- DE with all active RGB zero; or
- DE with at least one nonzero active RGB sample.

Three mutually exclusive event toggles crossed to `clk_sys`; the host compared
two short snapshots to determine whether the final raster was advancing.

#### First implementation — `de0024fb`, reported by `bb33351a`

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.164 ns |
| Hold slack | 0.242 ns |
| TNS | 0 |
| Resource delta | +113 ALMs / +156 registers |
| Observer hierarchy | 104.7 ALMs / 123 registers |

The worst path was unrelated `ascal` logic. The observer nevertheless exceeded
the 96-register ceiling because separate eight-bit epochs and streamed snapshot
state fitted larger than expected.

#### Compact epochs — `34dba1e5`

The correction reduced epochs to four-bit modulo counters, shortened the host
sample interval, reused each counter LSB as the last-seen toggle, and packed
the activity counters into the existing snapshot bank.

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.221 ns |
| Hold slack | 0.244 ns |
| TNS | 0 |
| Resource delta | +127 ALMs / +150 registers |
| Observer hierarchy | 94.4 ALMs / 155 registers |

Setup recovered above the then-current 0.200 ns floor, but fitted register use
still failed. Source-level bit removal did not predict fitted register packing.

#### Field evidence gained

Despite its signoff history, the qualified field recorder later captured the
key black-screen boundary:

- final HDMI VS and DE continued for three frames;
- every active final RGB sample was zero;
- the FPLL was locked with no new loss;
- the MagiK framebuffer route was active and accepted;
- the exact scanout memory contained the correct colourful 960x600 image; and
- latch post, flip, and drop evidence was coherent.

This ruled out an external-only transmitter, cable, capture-device, or display
failure. The FPGA itself was registering black pixels. See
[v0.26 final-output black-screen evidence](2026-08-13-v026-final-output-black-screen.md).

### Stage C: mux, raw scaler, post-processing, and Avalon evidence (`0x62–0x65`)

#### Aim

To avoid another occurrence requiring another FPGA build, the next milestone
added several narrow boundaries in one candidate:

- cycle-aligned final-mux provenance;
- raw `ascal` frame classification;
- post-OSD frame classification; and
- bucketed Avalon request, accepted-request, returned-data, and clock-activity
  epochs.

Pixel buses were reduced locally to one-bit frame classifications; only event
toggles crossed domains. The host required stable multi-frame deltas and did
not treat independently synchronized domains as one coherent frame.

#### Incremental corrections

The observer underwent several bounded semantic and implementation fixes:

- heartbeat activity became a modulo bucket epoch rather than a sticky bit, so
  an advancing empty Avalon window could be distinguished from a stopped
  clock;
- the sampling interval was bounded below the modulo-alias interval;
- raw and post-processing claims required at least two frames rather than one;
- unsupported/reserved fields and CRCs were strictly decoded;
- command snapshots included coincident `_next` events and remained immutable
  during streaming; and
- the local/GitHub cache identity and processor policy were made deterministic.

#### Evidence gained

During the next black-screen capture the expanded evidence showed:

- the final mux selected the scaled path, not the intentionally black direct
  path;
- raw `ascal` produced DE but zero active RGB;
- post-processing and final output were therefore not the first black boundary;
- `clk_100m` continued advancing; and
- request, accept, and return activity had existed earlier, then all three
  bucket epochs stopped while the clock bucket continued.

That localized the failure to the scaler fetch scheduler/return-credit path,
not the framebuffer contents, latch route, direct-video mux, post-OSD logic,
physical FPLL, or downstream HDMI path.

The same evidence did **not** prove the cause of the vertical colour bands.
There was no authoritative one-bit line-underflow signal at the scaler consumer
boundary. A transient `readlev=2, copylev=0` condition can be normal memory
latency, so it could not honestly be reported as a vertical-band fault.

### Stage D: lossless scaler-completion repair and `0x66` health evidence

#### Root-cause mechanism

Review of `ascal.vhd` found a concrete lossy CDC mechanism:

1. `ascal` allows two output framebuffer-read blocks to be outstanding.
2. The Avalon domain toggled one completion bit once per returned 128-beat
   block.
3. The HDMI domain detected only `sync XOR previous_sync`.
4. If `clk_hdmi` stopped during PLL reconfiguration while both reads completed,
   the source bit toggled twice and returned to its original value before the
   destination sampled it.
5. On resume, the destination saw no completion. `o_copylev` remained zero,
   `o_readlev` remained two, and the scheduler could issue no more reads.

This exactly explains a live raster with black pixels, earlier framebuffer
activity, and a later interval with no requests, accepts, or returns. It was
proved possible in a deterministic test at `e1fbe044`; the preserved hardware
capture did not contain the internal state needed to prove that this exact
sequence occurred in that incident.

#### Repair evolution

The functional repair was kept separate from diagnostic claims:

1. `4e6dc039` replaced the completion toggle with a registered two-bit
   modulo-4 Gray completion counter.
2. `8638a59b` serialized a skipped delta of two into two ordinary one-credit
   events, preserving the legacy copy-level truth table and metadata order.
3. The source Gray value was explicitly registered so binary carry glitches
   could not cross as Gray.
4. Reset handling was made asynchronous-assert/synchronous-release in the
   destination domain.
5. Delta three, copy-level overflow, and underflow were rejected or retained as
   sticky evidence rather than fabricating credits.
6. The exact two source-Gray-to-first-stage paths received a nonempty 10 ns
   net-delay bound; no functional false path or multicycle exception was added.
7. `0x66` was designed to expose stable fetch-health state, batch-two recovery,
   and full-frame starvation without exporting raw multibit state across
   domains.

#### Review-driven corrections

The implementation was repeatedly tightened before synthesis:

- invalid or incoherent `0x66` snapshots were forced to zero so the strict ABI
  could not accept stale payload;
- the reset-release synchronizer was explicitly identified and counted;
- the full-frame starvation accumulator was fixed so reset zero was not an
  absorbing state;
- weak line-starvation telemetry was removed because it could not prove the
  vertical-band mechanism;
- retired ABI positions remained strict zero rather than being reinterpreted;
- the real production integration harness exercised the completion repair; and
- the cache stopped invalidating invariant variants merely because the wall
  date changed, while one pinned synthesis epoch remained part of the candidate
  identity.

### Stage E: implementation-size and timing recovery attempts

The combined narrow observer was much smaller than Design 1, but still changed
the dense legacy scaler's global placement. Several attempts reduced local
logic without changing the ABI or functional repair.

#### Compact snapshots — `467ded35`

Snapshot registers were shared where layouts allowed it. This retained the
public records but did not remove enough global fitted state.

#### Packed snapshot payload — `3b277124`, reverted by `33acaac2`

The snapshot payload was packed more aggressively. Quartus inferred very large
response muxes, including 130:1 and 257:1 structures. Register count fell, but
ALMs and placement became worse. The change was explicitly reverted.

The lesson was that register-bit minimization can increase mux depth and routing
pressure; it is not automatically a physical reduction.

#### Shared command decode and shortened counters — `85687a72`, `1fe989c0`,
`0382ac3b`

The seven evidence opcodes shared a three-bit selector, response selection was
made word-major, and modulo counter carry chains were shortened algebraically.
The candidate reduced ALMs but failed timing:

| Measurement | Restored candidate | Decode/counter experiment |
| --- | ---: | ---: |
| Setup slack | 0.400 ns | 0.252 ns |
| Hold slack | 0.222 ns | 0.241 ns |
| Resource delta | +338 ALMs / +270 registers | +316 ALMs / +299 registers |

The worst path was wholly inside legacy `ascal`, from `o_vcpt_pre3[1]` to
`o_fload[0]`. Fewer observer ALMs produced worse global timing and more fitted
registers. All three optimization commits were reverted. See
[FPGA diagnostic decode timing experiment](2026-08-14-fpga-diagnostic-decode-timing-experiment.md).

### Stage F: architectural isolation of the repair

After the Boolean optimizations failed, the design was reduced by architecture
rather than by further expression tuning.

#### External repair plus exact `0x60` — `b8c36bc1`

The broad `0x61–0x66` hardware was removed. The known `0x60` lock responder was
kept independent, and the completion repair was isolated from the latch.

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.353 ns |
| Hold slack | 0.249 ns |
| Resource delta | +92 ALMs / +151 registers |

This was dramatically smaller but still failed the 0.428 ns setup requirement
and 96-register ceiling.

#### Internalized completion repair — `2e619b94`

The Gray counter and destination consumption were moved wholly inside `ascal`,
removing the `sys_top` round trip and external repair module. `0x60` remained.

| Measurement | Result |
| --- | ---: |
| Setup slack | 0.204 ns |
| Hold slack | 0.243 ns |
| Resource delta | +67 ALMs / +148 registers |

The mapped functional repair itself was only four registers and the exact lock
responder was 65 registers. The much larger fitted delta came from global
duplication and packing. The worst path was unrelated OSD logic.

#### Repair only; remove `0x60` — `754f70ed`

Because the captured failures had already ruled out a new FPLL unlock and the
user authorized retiring it, the final factorial experiment removed all FPGA
diagnostic commands. The `0x60–0x67` compatibility source paths compiled to zero
entities. Only the internal Gray completion repair and its exact two timing
paths remained.

Stock and pinned baseline were cache hits; only the patched variant rebuilt.
The result was:

| Measurement | Result | Gate |
| --- | ---: | ---: |
| Setup slack | 0.072 ns | at least 0.428 ns |
| Hold slack | 0.190 ns | at least 0.200 ns |
| TNS | 0 | 0 |
| Unconstrained outputs | 158 / 158 | equal |
| Resource delta | **-27 ALMs / +84 registers** | at most +150 / +96 |
| Calculable synchronizers | baseline 5, patched 7 | exactly +2 |
| Gray net-delay minimum slack | 8.855 ns | nonnegative |

Removing `0x60` solved the resource gate. It did not solve physical closure.
The worst setup path was unrelated SDRAM-to-configuration logic:

```text
emu|sdram|data[8] -> emu|cfg[2]
```

The worst hold path was unrelated legacy scaler polynomial memory-to-DSP logic:

```text
ascal|o_poly_phase_b.t2[5] -> ascal|o_poly_phase_b2.t2[5]
```

No completion-repair or diagnostic node was on either path.

### Why Design 2 failed as a releasable architecture

Design 2 was far better scoped than Design 1 and generated decisive field
evidence. The lock-only milestone itself worked. The final-output and path
observers localized the black screen, and the source review found a credible,
test-reproduced completion-credit defect.

It nevertheless failed as a releasable combined solution:

1. **Every evidence expansion exceeded either timing or fitted-register
   policy.** Local source reductions did not predict the fitted result.
2. **The merged observer affected global placement.** It grew both the sidecar
   and latch transport, retained many event and snapshot registers, and loaded
   several dense scaler/video boundaries.
3. **The functional repair alone still changed fixed-seed placement enough to
   fail.** The final repair-only build passed resources and CDC but failed both
   setup and hold on unrelated legacy paths.
4. **The vertical-band fault remained unproven.** The available evidence could
   localize black-frame starvation, but no low-cost, authoritative line-buffer
   consumer invariant was found. Reporting normal fetch latency as underflow
   would have been misleading.
5. **Timing was non-monotonic.** Across the final three isolated builds, removing
   logic improved resources while setup moved from 0.353 to 0.204 to 0.072 ns.
   This rules out further selector or bit-packing work as an evidence-led timing
   strategy.

The final repair-only RBF is invalid and must not be installed or published.

---

## Facts established despite the failed implementations

The work was not diagnostically empty. It established the following facts:

1. A captured black-screen occurrence did not add a physical HDMI FPLL loss.
2. The authoritative MagiK framebuffer contained the correct nonblack image.
3. The FPGA latch route was active, accepted, and not dropping posts.
4. Final HDMI VS and DE continued while every active final RGB sample was zero.
5. The final mux selected the scaled path, not the intentionally black direct
   path.
6. Raw scaler output was already black, before post-OSD and final staging.
7. The 100 MHz memory-side clock continued after request, accept, and return
   activity stopped.
8. The legacy one-bit completion toggle can provably lose two completions while
   `clk_hdmi` is stopped, leaving the two-entry read scheduler permanently
   saturated.
9. The two-bit registered Gray repair behaves correctly in focused simulation,
   but the present FPGA integration cannot be qualified under the fixed-seed
   physical gates.
10. The vertical-colour-band occurrence may involve partial line integrity or
    fetch starvation, but the captured evidence and attempted diagnostics did
    not prove that it shares the black-screen mechanism.

## Final disposition

- Design 1 is retired permanently. Do not restore its wide three-domain
  snapshot, bundled payload CDC, or `0x5d–0x5f` hardware.
- Design 2's old software decoders remain only for already-qualified rollback
  RBFs. The repair-only candidate intentionally implements none of
  `0x60–0x67`.
- No rejected RBF in this history is release-qualified.
- Do not continue with seed sweeps, timing waivers, placement directives,
  selector reshaping, snapshot packing, or unrelated legacy-path edits.
- Any future attempt needs a new architectural boundary and its own predeclared
  physical gates. The current evidence specifically argues against adding more
  observer fabric around the dense legacy scaler.

Related retained evidence:

- [Seed-2 wide-observer retirement](2026-08-12-fpga-seed-2-video-diagnostics-retirement.md)
- [Lock-high black-screen capture](2026-08-13-hdmi-lock-high-black-screen.md)
- [Final-output black-screen capture](2026-08-13-v026-final-output-black-screen.md)
- [Final-output activity signoff](2026-08-13-fpga-final-output-activity-signoff.md)
- [Build-identity correction](2026-08-13-fpga-signoff-build-identity.md)
- [Decode timing experiment](2026-08-14-fpga-diagnostic-decode-timing-experiment.md)
