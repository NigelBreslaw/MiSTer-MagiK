# FPGA video diagnostics and scaler-return recovery design history

Date: 2026-08-14

This document records the two FPGA diagnostic designs developed to investigate
the rare Arcade-to-MiSTer-MagiK return failures:

- a black screen with live software and framebuffer content; and
- a separate vertical-colour-band failure.

It records the aims, architecture, incremental corrections, measured results,
and the evidence-led reasons neither diagnostic design was a releasable final
solution. It also records the subsequent repair-only architecture that passed
the local RTL, formal, CDC, timing, and resource gates. It is history, not
current implementation policy.

The two designs are:

1. the original wide, three-domain first-fault snapshot observer; and
2. the replacement staged observer, which began with the small `0x60` lock
   recorder, expanded through final/scaler/Avalon evidence, and ultimately
   incorporated a lossless scaler-completion repair.

The successful follow-on repair is recorded separately as Design 3 because it
retired the Gray counter and all production observer opcodes rather than
continuing either diagnostic architecture.

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

## Design 3: queued one-bit completion transport with reset-safe returns

### Selection and scope

The post-retirement review compared the failed two-bit Gray counter, a
reset/quiesce controller, per-slot completion toggles, and a queued one-bit
request/acknowledgement transport. The queued transport was selected because
it kept the established destination-domain completion pulse, copy-level truth
table, metadata order, and single-bit forward crossing while moving the new
queue work into the Avalon clock domain.

The design deliberately introduced no production video observer, new UIO
opcode, PLL controller, framebuffer route, pixel-output cone, or reset-only
recovery transaction. Commands `0x60–0x67` remain unsupported in the
repair-only RBF. Physical HDMI visibility therefore remains an external
qualification responsibility rather than an FPGA self-report.

The implementation began at `edf2cff6` and was completed by the forward
corrections through `a524d551`. The proof integration was completed by
`621c8af2` and `a0768d04`. No rejected Gray commit was amended or rewritten.

### Queued completion request/acknowledgement

The source side retains the legacy one-bit completion request toggle and adds:

- `completion_pending`, a capacity-one queued event behind the active toggle;
- `completion_ack_meta`, the first reverse acknowledgement synchronizer stage;
  and
- `completion_ack_sync`, the second reverse acknowledgement synchronizer stage.

Together, the active request and pending bit preserve the two completions that
the scheduler can legally have outstanding. The exact source transition is:

```text
completion = one complete BLEN block returned
idle       = request_toggle == synchronized_destination_seen

if idle and pending:
    toggle request
    pending = completion
else if completion:
    if idle:
        toggle request
    else if not pending:
        pending = 1
    else:
        assert unreachable queue overflow
```

A completion coincident with acknowledgement and pending forwarding therefore
replaces the forwarded pending event rather than being lost. The HDMI domain
retains the registered `sync XOR sync2` pulse and the established `o_copylev`
update truth table. The reverse acknowledgement observes the stable
destination-seen toggle only after destination accounting.

### Reset-era Avalon return protection

Review then proved that the completion handshake alone was insufficient across
reset. `ascal` observes `reset_req` before the system-memory terminator has
finished its synchronized reset transition, and the terminator forwards
eventual `readdatavalid` beats without cancellation or an epoch tag. A response
accepted before reset could therefore arrive after the scaler state had been
cleared.

The first retained-return implementation counted remaining return words. It
was corrected in forward commits after review exposed three separate issues:

1. charging at request assertion or inferred acceptance could create an
   obligation for a request that was later cancelled;
2. an `avl_read_i` held through reset could be accepted repeatedly while the
   memory-side reset was still propagating, or accepted as an orphan immediately
   after scaler reset release; and
3. unconditional VS phase realignment could reset `avl_wad` while a legal
   accepted burst was still returning, producing an early or late completion.

The final implementation uses retained radix accounting:

- return credits in the range `0..2` count accepted BLEN bursts;
- a phase in `0..BLEN-1` counts returned beats within those credits;
- the pair is retained through scaler reset;
- old returned beats reduce the retained obligation while drain is closed, but
  cannot write scaler RAM or create completion credits; and
- the drain opens only on a later synchronized VS after the retained credit and
  phase are both empty.

The retained `avl_read_accepted` guard charges the accounting only on the first
actual eligible Avalon handshake. Public request and acceptance share the exact
eligibility condition:

```text
avl_read_i
and not avl_read_accepted
and (reset active or scheduler state is sREAD)
```

The guard is not cleared by scaler reset while `avl_read_i` may remain high. It
suppresses repeated acceptance during reset and clears only after the internal
request drops. Once reset releases into `sIDLE`, the scheduler-state condition
immediately blocks an unaccepted orphan before it can handshake.

VS phase realignment is likewise permitted only when retained return accounting
is empty. A VS coincident with the final old beat waits for the next VS. This is
fail-closed: no HDMI clock or VS means the new epoch is not exposed.

### Exact-source verification

The production patch contains the shared queue, return-accounting, phase, and
drain transition functions used by synthesis and GHDL verification. Structural
checks bind the exact patch locations, reset topology, synchronizer stages,
Avalon eligibility, accepted-read guard, drain masking, and VS guard. A mirrored
SystemVerilog implementation or source-text match alone is not accepted as the
proof.

The final narrow proof model reproduces production asynchronous reset assertion
and synchronous per-domain release. Its responder is independent of the DUT
credit counter: it creates obligations only from actual accepted requests,
allows arbitrary waitrequest and return stalls, retains old obligations across
reset, and forbids only a zero-latency first DDR response when no command was
outstanding. That restriction is derived from the exact HPS F2SDRAM topology;
there is no combinational path from a newly accepted read to
`readdatavalid`.

Fixed-cycle formal guides were replaced with event-driven witnesses based on
actual acceptance, return, reset, acknowledgement, and VS events. The final
gate passed:

- exact production patch/source binding;
- GHDL analysis, elaboration, testbench execution, and synthesis;
- reset-reachable bounded model checking through 24 global interleavings;
- all nine required non-vacuity covers;
- stopped HDMI clock with two completions and ordered delivery;
- coincident acknowledgement and completion;
- final and subsequent old return beats during reset/drain;
- VS-only drain release and first correctly phased post-drain completion;
- active-credit VS, issue plus empty VS, and final-return plus VS races; and
- temporal induction with a maximum depth of 32, closing at induction length
  10.

The induction strengthening consists of asserted production-derived range and
two-flop pipeline coherence invariants. They are proved properties, not free
environment assumptions. The final proof used the patched production
`ascal.vhd` SHA-256
`aa9fb1353652aef34dabc7f9f614539fabd2e6228b35352aa28c1905f6d41cab`.

### Physical closure evolution

The first queued-handshake/reset-drain implementation built successfully but
was rejected:

| Measurement | First queued candidate | Gate |
| --- | ---: | ---: |
| Setup slack | -0.352 ns | at least 0.428 ns |
| TNS magnitude | 0.697 ns | 0 |
| Resource delta | +162 ALMs | at most +150 |

The failing setup path was unrelated SDRAM data-to-configuration placement.
No timing waiver, seed sweep, false path, placement directive, or fitter-setting
change was accepted. The retained return counter was instead compacted from a
monolithic word count to credits plus phase, the separate alignment bit was
removed, direct legacy write/completion expressions were restored, and three
empty observer QSF assignments were removed. The functional reset and
completion protections remained intact.

The canonical local Apple-container signoff then passed on root `621c8af2`
using Quartus Prime Lite 17.0.0 Build 595, seed 2:

| Measurement | Final queued candidate | Gate |
| --- | ---: | ---: |
| Setup slack | **0.580 ns** | at least 0.428 ns |
| Hold slack | **0.249 ns** | at least 0.200 ns |
| TNS | **0** | 0 |
| Unconstrained outputs | **158 / 158** | equal to baseline |
| Resource delta | **+132 ALMs / -10 registers** | at most +150 / +96 |
| Block memory / DSP / PLL delta | **0 / 0 / 0** | unchanged |
| Recognized synchronizer chains | baseline 377, patched 379 | exact +2 |
| Calculable synchronizer chains | baseline 5, patched 7 | exact +2 |
| Forward/reverse net-delay rows | **1 / 1** | exactly one each |

Quartus reports the pre-existing forward crossing and the new reverse crossing
at greater than one billion years MTBF each. Combining their failure rates
conservatively gives approximately 500 million years, above the required
`10^12` device-hours. The checker was corrected to understand Quartus's capped
`Greater than 1 Billion` wording and the fact that explicit attributes made the
existing forward crossing newly calculable; this changed evidence parsing, not
the MTBF or topology requirement.

The matched local RBF SHA-256 is
`7484e004b3c6e089d9d377658633e435703bc1a224943b06215df9a9bccef4e7`.
Its metadata SHA-256 is
`3e439a664e15ef48ef69f1663a818ca052e223ce0e22444a6b249a1dd70ebc39`,
and the valid Quartus delta report SHA-256 is
`38c2bac69ad98c151fd7da11b971bb3db082d9ee6fc21fb1d6fb7bc195306b70`.
Later proof-only commits do not alter the production patch or RBF inputs.

### Main/runtime fail-closed integration

The repair is functional FPGA logic; software readiness is not presented as a
substitute. The associated root runtime and maintained Main fork changes close
activation, ownership, false-ready, and recovery paths around the repaired RBF:

- repair-only activation accepts explicitly unsupported retired observer
  commands only with exact latch-v5 protocol/capability and platform identity;
- `ready-v2` binds child token/PID, Main PID/generation, UIO owner epoch, route,
  geometry, two advancing completed posts, alternating slots, receipt CRCs,
  visible-row RGB565 SHA-256, and nonzero-pixel count;
- Main rejects stale identities, routes, epochs, duplicate/same-slot posts,
  malformed records, and invalid latch evidence;
- the first failure records an incident and permits exactly one fresh-child
  retry after ownership recovery;
- the second failure restores stable stock UI and input rather than looping,
  resetting the core, reloading the RBF, or cycling HDMI; and
- terminal fallback rearms readiness only after recovery, so a later independent
  launch again receives its own bounded retry.

The relevant root commits are `de1ad382` and `d23770f1`. The maintained Main
fork changes are `7c050d1` and `4566825`. Source-frame digest and completed
latch posts prove software/transport coherence, not physical HDMI visibility.

### Qualification disposition

Design 3 is the selected black-screen repair and has passed local RTL, exact
formal, CDC, timing, hold, TNS, resource, clock-relationship, and MTBF gates.
Unlike Designs 1 and 2, its matched local RBF is eligible for the attended,
rollback-capable Dev installation transaction.

It is not yet a commercial release solely from those results. Release still
requires output-rate lossless HDMI/CRT capture, bounded Dev smoke, the declared
three-board/two-chipset/300,000-transition campaign, the long frame/latch gate,
and canary evidence on one unchanged production tuple. Any physical black,
stale, partial, corrupted, or vertically banded frame rejects the candidate.
The vertical-band symptom is not declared fixed by the completion CDC proof.

---

## Design 4: minimal scaler-scheduler attribution (`0x67`)

The queued repair reduced the apparent failure rate but did not eliminate the
physical black screen. On 2026-08-23 a lossless physical capture showed stable
video-level black while the internal RGB565 framebuffer remained correct and
owned vblank continued. The repair-only RBF could not expose the internal
scheduler state because its historical diagnostic fields were compatibility
stubs.

Design 4 is an experimental attribution candidate, not another functional
repair. It retains Design 3 bit-for-bit and adds one read-only 16-bit state
word assembled inside `ascal`. Seven Avalon-side state bits cross through
explicit two-stage synchronizers. The HDMI domain publishes the word only
after two identical completed-frame samples; a generation-toggle bundled-data
crossing then transfers the stable word to the independent `clk_sys` responder.

The record contains only run state, per-frame read/completion activity,
read/copy levels, completion request/pending/acknowledgement state, destination
observation, and retained return drain/credit/phase state. It does not tap RGB,
DE, PLL, route, address, framebuffer, mux, reset-control, or transmitter cones.
Command `0x67` returns magic `0x4d57`, schema 1, the immutable state word, and
CRC-16. Commands `0x60` through `0x66`, latch-v5, and capabilities `0x03ff`
remain unchanged.

Three identical coherent records are required for classification. Backlogged
request/ack state, idle `readlev=2`/`copylev=0` credit loss, and continuing
scheduler progress are distinct outcomes; malformed or changing evidence is
inconclusive. The observer remains experimental until exact simulation,
formal-regression, fixed-seed Apple-container timing/CDC/area signoff, and
attended physical smoke all pass.

### Local candidate result and experimental path-count exception

Candidate `612a04a0f` encodes the reset-return crossing as drain-inactive and
restores the public drain-active meaning only in the HDMI-domain packing
function. This keeps the crossing on a normal one-source data route without an
extra register or an observer primitive. GHDL production-patch simulation,
Icarus latch/sys-top simulation, and exact-source formal proof passed with all
required covers.

The canonical Quartus 17.0 Build 595 seed-2 run produced setup slack `0.660 ns`,
hold slack `0.249 ns`, zero TNS, `+131` ALMs, `+89` registers, unchanged
RAM/DSP/PLL identity, all 26 expected diagnostic CDC paths, and MTBF above the
fixed gate. Its only original rejection was 160 unconstrained output-port
paths versus the matched baseline's 158. The operator explicitly accepted the
two-path delta for this experimental diagnostic RBF. The verifier records that
exception as `diagnostic_unconstrained_output_paths_exception=true`; it does
not waive added unconstrained identities or any timing, CDC, MTBF, resource,
warning, or device-identity gate. This exception does not qualify the RBF for
production release.

### Invalid first phase-2 attempt

The attended phase-2 campaign reproduced a persistent physical black screen on
the third valid transition with this exact candidate. Native USB video was
uniform black while the authoritative RGB565 framebuffer remained correct and
nonblank. The initial failure capture and two later read-only captures produced
nine identical, coherent `0x67` samples with state word `0x0da3`:
`readlev=2`, `copylev=2`, request/acknowledgement/destination toggles aligned,
no pending completion, and no retained reset-return credits or phase.

This rules out completion-queue backlog and the specified idle-credit-loss
state for the captured occurrence. Design 4 has therefore completed its narrow
attribution purpose and must not be expanded in place. The exact result and
evidence hashes are recorded in
[`2026-08-23-scaler-scheduler-black-screen-result.md`](2026-08-23-scaler-scheduler-black-screen-result.md).
The next authorized experiment should use a separate minimal raw-scaler
boundary probe, preserving Design 3 and latch-v5 unchanged.

## Design 5: disposable raw-scaler boundary attribution (`0x67` schema 2)

Design 5 implements the decision from the captured `0x0da3` incident. It
removes Design 4's scheduler functions, ports, HDMI process, seven-bit reverse
observer synchronizer, and scheduler ABI. It retains the queued completion
repair unchanged.

One SystemVerilog observer beside `sys_top` reads only the existing raw scaler
RGB, DE, HS, VS, and clock-enable wires. Per completed frame it publishes a
modulo-16 heartbeat, saturating active and nonzero sample counts, HS/CE state,
and validity in one 16-bit word. One registered generation toggle and stable
bundled word cross to the existing read-only responder. This makes a stopped
HDMI/scaler clock observable as a stale frame sequence, correcting Design 4's
inability to distinguish a coherent last sample from fresh identical state.

The host requires three CRC-valid samples and classifies stopped timing,
missing active video, active all-zero raw output, sparse/corrupt output, or
substantial nonzero raw output. A single flashing pixel cannot qualify as a
healthy raw image. The complete frozen ABI and diagnostic-only gates are in
[`docs/fpga-raw-scaler-diagnostic.md`](../docs/fpga-raw-scaler-diagnostic.md).

Before synthesis, patched production GHDL compilation, queue simulation,
sys_top/latch simulation, the raw-boundary Icarus suite, all 41 Quartus checker
fixtures, exact-source BMC, nine non-vacuity covers, and temporal induction
passed. Physical and Quartus results remain to be appended against the frozen
candidate identity.

The first exact Design 5 Quartus build exposed and rejected one retired
Design 4 Avalon diagnostic register that remained assigned but unread. Commit
`dee39545b` removed only that dead state; patched-production GHDL and Icarus
integration simulations passed again before synthesis. The corrected seed-2
build has zero added warning classes, zero TNS, setup `0.389 ns`, hold
`0.216 ns`, `+109` ALMs, `+55` registers, unchanged RAM/DSP/PLL identity, all
19 exact diagnostic CDC paths, the required three calculable custom chains,
and per-chain MTBF above `10^12` device-hours.

That result is deliberately not production-qualified: it misses the production
`0.428 ns` setup floor and `0.150 ns` matched-baseline degradation limit, and
Quartus's placement-sensitive aggregate automatic synchronizer count changed.
For this removable observer only, local Apple-container signoff uses a named
`experimental_raw_scaler` profile with a `0.350 ns` setup floor and `0.300 ns`
degradation ceiling. It may ignore only the aggregate chain total; exact
assignments, calculable-chain delta, endpoint delay reports, MTBF, hold, TNS,
warnings, resources, and device identities remain mandatory. The production
checker defaults used by CI remain unchanged.

### Physical attribution result

The exact Design 5 candidate was installed through the attended agent-first,
rollback-capable transaction. Baseline USB video was complete and internal
evidence classified `raw_scaler_active`. The first typed Arcade automation did
not launch a core and timed out. A subsequent clock-only USB still was
incorrectly called the target black-screen failure; the physical operator
rejected that classification.

Two subsequent read-only bundles reported advancing `raw_scaler_active`, and
the authoritative framebuffer remained complete. Those records were not tied
to a valid launch/return failure and provide no black-screen attribution.

Design 5 remains installed and active. Correct only the phase-2
launcher/catalog harness, then run valid bounded return transitions until an
operator-confirmed failure is captured. Do not retire or replace the observer
based on this invalid attempt. The exact correction is recorded in
[`2026-08-23-raw-scaler-boundary-black-result.md`](2026-08-23-raw-scaler-boundary-black-result.md).

### Operator-confirmed moving-band result

The corrected harness completed 55 uninterrupted valid returns without a
black result. A subsequent attended reboot completed and immediately exposed
the rare moving-band corruption on fresh MagiK Home. The physical 1920x1080
output contained continuously descending spatial discontinuities while the
authoritative latched RGB565 framebuffer remained complete and correct.

Two independent read-only snapshots retained coherent `raw_scaler_active`,
stable `LauncherActive` ownership, latch presentation status `ok`, and zero
drops. A 30-second native movie and all 732 delivered frames prove that the
band moves from top to bottom and wraps repeatedly. Design 5 therefore answers
only that substantial nonzero raw activity continues; its saturating activity
counters cannot prove line count, sync ordering, active width, or phase.

This is valid attribution evidence, but not a final root cause and not proof
that the moving-band and persistent-black mechanisms are identical. The
device remained unrecovered while the complete 793-artifact integrity manifest
was created. See
[`2026-08-23-moving-band-corruption-result.md`](2026-08-23-moving-band-corruption-result.md).

### Direct-Arcade transient-corruption result

The later direct-Arcade campaign removed the irrelevant SNES navigation while
retaining the same fixed game and launch/handoff/typed-return boundary. It
added an explicit two-second in-game dwell and three physical MagiK samples.
After 14 valid passes in boot epoch 4, the middle sample was corrupt while the
samples before and after were byte-identical healthy output.

The latched RGB565 framebuffer stayed correct. Failure-time and subsequent
read-only records were coherent `raw_scaler_active` with zero latch drops or
rejects. A native 30-second follow-up contained 755 healthy frames, proving the
sampled corruption self-cleared before the movie and was not the earlier
continuous moving-band state.

Design 5 again shows that substantial raw activity continues during physical
corruption, but its activity counters cannot identify timing, geometry, or
sync-order faults. No black-screen attribution follows from this event. The
unrecovered-state record is in
[`2026-08-24-phase2-transient-corruption-result.md`](2026-08-24-phase2-transient-corruption-result.md).

## Design 6: sticky raw-scaler frame-integrity recorder

The one-frame 2026-08-24 event self-cleared before the 30-second movie, while
Design 5 continued to report saturated raw activity. Design 6 therefore removes
the RGB/activity observer rather than widening or stacking it. The replacement
reads only raw CE, DE, HS, and VS, establishes a three-identical-frame baseline,
and retains the first differing ordered-control CRC until the
existing reset or RBF reload.

Command `0x67` becomes `raw-scaler-frame-integrity-v1`, schema 3. Commands
`0x60–0x66`, latch protocol 5, capabilities `0x03ff`, the completion repair,
and all production routing/reset/pixel cones remain unchanged. The fixed record
contains baseline and first-bad control CRC. It has
no write, clear, arm, freeze, or recovery operation and never claims physical
sink visibility.

The first synthesis of the wide schema-3 record was rejected at commit
`53ba7c96a`: hold slack was 0.174 ns, with 397 ALMs and 661 registers above the
matched baseline. The forward candidate removes the duplicated count records
and keeps the 0.200 ns hold floor. This is a causal reduction, not a waiver or
seed change.
The subsequent 48-bit fit reached 0.516 ns setup, 0.224 ns hold, zero TNS, 165
registers, and unchanged hard-block identity, but exceeded the frozen
experimental ALM cap by six. Redundant observer state and the generic response
selector were removed before the next fixed-seed build; the cap was not waived.
That smaller form reduced growth to 131 ALMs and 123 registers but also reduced
setup slack to 0.250 ns and was rejected. The prior control form is restored
with an explicit 208-ALM disposable-diagnostic ceiling for its measured 206
ALMs; its 0.516 ns setup, 0.224 ns hold, zero TNS, and 165-register result are
the safer fixed-seed tradeoff.

The design and pre-synthesis gates are frozen in
[`2026-08-24-raw-scaler-frame-integrity-design.md`](2026-08-24-raw-scaler-frame-integrity-design.md).

## Design 7: completed-frame raw RGB state

Design 6 reproduced a persistent physical black while CE/DE/HS/VS stayed at
the healthy baseline. Design 7 therefore removes the schema-3 control CRC and
does not stack another observer. It reads only the direct ascal RGB output plus
raw DE and VS, publishes the most recently completed active frame, and retains
only any-nonblack, variation, and the exact first 24-bit RGB sample.

The production boundary is unambiguous: `hdmi_data[23:0]` maps as 8-bit R, G,
and B with exact black `0x000000`. Command `0x67` becomes
`raw-scaler-rgb-state-v1`, schema 4. The record stays five words, has no
write/clear/arm/freeze behavior, and never infers sink visibility. Healthy
activation accepts only three coherent `raw_rgb_varied` samples; black,
constant, or inconclusive evidence fails closed.

The frozen design and next-result decision are in
[`2026-08-24-raw-scaler-rgb-state-design.md`](2026-08-24-raw-scaler-rgb-state-design.md).

## Design 8: completed-frame scaler pipeline state

Design 7 reproduced a persistent physical black screen while the authoritative
framebuffer stayed correct and three coherent completed active frames reported
exactly black raw scaler RGB. Design 8 therefore removes the schema-4 RGB
observer rather than stacking it and follows the minimum internal stage ladder:
accepted read, current/nonzero Avalon return, completion creation/delivery,
copy/nonzero DPRAM word, line-buffer write/nonzero pixel, and active/nonzero raw
output.

Command `0x67` becomes `scaler-pipeline-state-v1`, schema 5. A second state word
retains the existing two-entry levels, queued completion handshake, reset-return
drain/accounting, scaler run/new-resolution, and read-active context. Three
valid completed-frame records select only the earliest absent or zero stage;
harmless queue phase changes do not make otherwise coherent evidence invalid.
Healthy activation accepts only full pipeline activity. No classification
claims physical visibility.

The design and exact encoding are in
[`2026-08-24-scaler-pipeline-state-design.md`](2026-08-24-scaler-pipeline-state-design.md).
Quartus 17 rejected the initial VHDL form because it read ascal OUT ports. The
forward form keeps internal flags 1 through 9 in ascal and merges only raw
active/nonzero flags from a preserved external HDMI boundary stage; it changes
neither port modes nor production output cones.

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
   but it cannot be qualified under the fixed-seed physical gates and is
   retired.
10. The queued one-bit request/acknowledgement repair preserves both legal
    outstanding completions, drains accepted pre-reset returns, and passed the
    exact-source formal and canonical local physical gates.
11. The vertical-colour-band occurrence may involve partial line integrity or
    fetch starvation, but the captured evidence and attempted diagnostics did
    not prove that it shares the black-screen mechanism.

## Final disposition

- Design 1 is retired permanently. Do not restore its wide three-domain
  snapshot, bundled payload CDC, or `0x5d–0x5f` hardware.
- Design 2's old software decoders remain only for already-qualified rollback
  RBFs. The repair-only candidate intentionally implements none of
  `0x60–0x67`.
- Design 3 is the selected implementation. Preserve its queued one-bit
  completion transport, accepted-read guard, retained reset-return accounting,
  VS guard, and exact formal/Quartus gates as one qualification boundary.
- No rejected RBF in this history is release-qualified. The passing Design 3
  local RBF remains Dev-only until physical qualification completes.
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
- [Current scaler-return recovery design](../docs/fpga-scaler-return-recovery.md)
- [Current physical return qualification](../docs/return-video-qualification.md)
