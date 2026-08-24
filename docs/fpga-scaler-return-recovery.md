# FPGA scaler return recovery design

Status: queued completion repair retained; copy-tail forward repair in local
proof; not release-qualified.

No retired diagnostic RBF is release-qualified. The measured attempts and
their disposition remain in
[FPGA video diagnostics: two attempted designs and their retirement](../history/2026-08-14-fpga-video-diagnostics-design-attempts.md).

Later passive evidence found a second causal defect after the queued completion
transport was already healthy: the final `sCOPY` horizontal carry can register
`o_last` on the last edge admitted by the legacy shift gate, stranding the
delayed line terminal and preventing `lev_dec_v`. The narrow forward repair is
documented in [Scaler copy-tail repair](fpga-raw-scaler-diagnostic.md). It
preserves the queued completion repair and removes the disposable observer.

## Decision

The next candidate replaces the lossy scaler completion toggle with a queued
one-bit request/acknowledgement handshake inside `ascal`. It keeps the legacy
HDMI-domain completion receiver and copy-level update structurally unchanged,
adds only a one-bit source queue and one two-register acknowledgement
synchronizer in `avl_clk`, and adds no FPGA diagnostic command or pixel-path
observer.

This is a functional repair, not a claim that the retained hardware capture
proved the exact internal state. The evidence proved that:

- the authoritative RGB565 source slot was correct and nonblack;
- latch ownership, posts, flips, and route geometry were coherent;
- final HDMI timing continued but active RGB was all zero;
- raw scaler output was already black;
- `clk_100m` continued while request, accept, and return activity stopped; and
- the existing completion toggle can deterministically lose two completions
  while `clk_hdmi` is stopped.

That makes completion loss a causal defect which must be repaired even though
the captured incident did not include `o_readlev` and `o_copylev`. The separate
vertical-colour-band failure remains an independent release hazard. It must be
absent from physical qualification; this design does not declare its root cause
solved.

## Review convergence

Independent RTL/CDC, Main/runtime, and qualification reviews challenged four
candidate boundaries:

| Candidate | Disposition | Reason |
| --- | --- | --- |
| Wide or staged production observer | Rejected | The observer fabric changed global placement and repeatedly failed timing or fitted-register gates. |
| Two-bit Gray completion pointer | Retired as the preferred repair | It is logically credible, but its added `o_clk` decode/seen state already failed canonical setup and hold with all observers removed. |
| PLL-lock/reset or scaler-epoch recovery | Rejected | It cannot cancel or tag late Avalon returns. A sound implementation needs a larger quiesce, drain, epoch, reset, prefill, and unblank controller across three domains. |
| Queued one-bit request/acknowledgement | Selected | It makes every completion durable while leaving new arbitration out of the dense HDMI scheduler cone. |

Per-buffer toggles were also rejected. They need two forward crossings and new
HDMI-domain popcount or serialization state, with no demonstrated physical
advantage over the failed Gray design.

## Completion transport

### Retained behavior

The repair retains:

- `avl_readdataack` as the one-bit completion request toggle;
- the existing two-register request synchronizer in `o_clk`;
- the existing registered XOR completion pulse;
- the existing `o_copylev` increment/decrement truth table;
- ordered Avalon returns and the existing request metadata FIFO; and
- the existing maximum of two outstanding output reads.

It adds only:

- `completion_pending`, a one-bit queue in `avl_clk`; and
- `completion_ack_meta` and `completion_ack_sync`, an exact two-register
  synchronizer returning the stable destination-observed request state to
  `avl_clk`.

The feedback source is the stable second destination synchronizer stage, never
the metastability stage or a combinational pulse. The implementation must prove
that a later request cannot reach the registered destination completion pulse
before the previous pulse has updated copy accounting. If that ordering cannot
be proved for the exact production VHDL, this architecture fails; an unreviewed
extra acknowledgement register is not added after synthesis merely to rescue
the candidate.

### Source transition rule

At the completed-block point:

```text
completion = one complete BLEN block returned in order
idle       = request_toggle == synchronized_destination_seen

if idle and completion_pending:
    toggle request_toggle
    completion_pending = completion
else if completion:
    if idle:
        toggle request_toggle
    else if not completion_pending:
        completion_pending = 1
    else:
        unreachable queue overflow
```

The `idle && completion_pending && completion` case emits the previously queued
completion and keeps the new completion queued. No same-edge event may be
silently overwritten.

If `clk_hdmi` stops while two reads complete, the first completion remains in
the request phase and the second remains in `completion_pending`. After
`clk_hdmi` resumes, the destination emits the first legacy completion pulse.
Its observed request state returns through the acknowledgement synchronizer,
which causes the source to emit the queued second request phase. The destination
then emits the second ordinary legacy pulse.

### Required invariants

The proof is over the actual patched VHDL and real scheduler, not only a mirrored
SystemVerilog model or textual source matcher:

1. The request toggle is stable until its destination-observed state returns.
2. Every completed returned block creates exactly one durable request or queued
   completion.
3. `busy && completion_pending && completion` is unreachable.
4. Acknowledgement, queued forwarding, and a new completion on one `avl_clk`
   edge preserve both events.
5. Accepted reads, complete returned blocks, destination pulses, metadata FIFO
   entries, copy starts, and copy retirements are conserved and ordered.
6. `o_readlev` and `o_copylev` remain in `0..2`; neither underflows nor
   overflows.
7. After any finite `clk_hdmi` pause, every retained completion is eventually
   delivered under fair clocks and Avalon service.
8. No new request can be detected before the preceding destination pulse has
   been accounted.

The third-completion proof must derive from production behavior. Issuing a
third read requires the oldest copy to retire. That retirement follows the
oldest completion pulse, which has already started the acknowledgement return.
The third request must then cross the existing request synchronizer, be accepted,
and return a full block. The acknowledgement must win that race for every legal
clock phase and minimum return interval. Treating “two outstanding” as a
testbench assumption is insufficient.

### Reset contract

The existing common reset remains the only reset authority. It asserts
asynchronously and releases synchronously in each `ascal` clock domain. Reset
must explicitly zero:

- the source request toggle and pending bit;
- both acknowledgement synchronizer registers;
- both destination request synchronizer registers; and
- the registered destination completion pulse.

Source-only or destination-only reset is illegal. The production integration
must prove that common reset also cancels or suppresses pre-reset sysmem returns;
no old Avalon response may be credited into a new reset epoch.

The current platform does not yet satisfy that last proof obligation.
`f2sdram_safe_terminator` intentionally lets an in-flight read finish across a
core-reset request and passes `readdatavalid` through without an epoch tag or a
drain acknowledgement. The queued completion repair does not hide this fact or
add an invasive reset controller. Until reset duration and drain ordering are
proven for the real platform, or a separately reviewed epoch design closes the
gap, reset qualification rejects this candidate.

## Production diagnostics and readiness

The production RBF contains none of commands `0x60` through `0x67`, no new UIO
responder, and no repair-health, PLL, pixel, route, or Avalon observer. The latch
and its protocol-v5 capabilities remain unchanged. Repair-local telemetry may
return only after a separate factorial build proves it placement-neutral; it is
not part of this design or a release prerequisite.

This deliberately separates correctness proof from field attribution. FPGA
observers which alter placement cannot qualify the production artifact they are
meant to describe.

The later experimental `scaler-scheduler-state-v1` attribution candidate is
not part of this production contract. It preserves this repair unchanged and
adds only read-only command `0x67`; `0x60` through `0x66` remain unsupported.
Promotion still requires removing or separately qualifying that observer after
the captured state identifies the remaining fault.

### Activation profile

Experimental and release activation must accept `0x60` through `0x67` being
unavailable. Activation instead requires:

- exact RBF, metadata, manifest, bundle, and component hashes;
- exact Main executable path plus fresh Main generation and PID;
- a fresh supervised launcher PID;
- latch protocol `5`, capabilities `0x03ff`, valid CRCs, status, receipt, and
  presentation telemetry; and
- stable Main ownership and owner epoch across preflight.

Malformed diagnostic responses or unexpected acknowledgements are failures;
an explicit unsupported result is correct for this candidate. The retired
`video-path-evidence-v1` dependency is replaced by the software-only
`scaler-completion-repair-v1` profile. That profile requires coherent latch-v5
capability, status, and presentation-telemetry CRCs plus current Main/launcher
identity and a stable owner epoch. It always reports
`sink_visibility: unobserved`.

### Fail-closed launch and return

Cold boot, game return, resume, active restart, and crash respawn retain one
ordered boundary:

```text
exact platform identity
→ Main owns UIO and holds native black with LFB disabled
→ Main reasserts the selected output mode on return
→ latch/platform preflight
→ ownership transfer
→ supervised child spawn with input disabled
→ cached source frame is locally nonblack and geometry-correct
→ two advancing alternating-slot posts
→ context-bound ready report
→ LauncherActive and input enable
```

The ready report is versioned and binds the token, child PID, Main generation
and PID, launcher PID, owner epoch, route and geometry, both sequences and
slots, latch identity/CRC/counters, and a renderer-side digest plus nonblack
statistic for the cached source frame.

This is `source_frame_ready`, never `visible`. Final VS, FPLL lock, ADV7513
presence, a nonblack hidden slot, and two latch posts each prove a useful layer;
none proves visible HDMI RGB. The captured failure already passed several of
those internal checks.

The existing eight-second deadline and one fresh-child retry remain bounded.
Before the first retry, Main must persist the incident capsule and restore
ownership. A second failure reaps the child, leaves Main owning the route, and
restores stable stock OSD/input over native black. Software must not infer a
black output and automatically reload the RBF, reset the core, power-cycle HDMI,
or reboot; without an RGB observer that trigger would be untrustworthy and could
create a recovery loop.

### Incident capsule

The first implementation writes one atomic, bounded current-incident record
before recovery mutates state. It already binds the failed attempt, token,
Main/launcher identities, owner epoch, reason, recovery selection, and explicit
unobserved sink state. The commercial completion target is a rotating record
which additionally contains:

- exact platform and component identities and hashes;
- boot, activation transaction, Main, launcher, PID, generation, and token
  identities;
- phase timestamps and ordered event tail;
- owner epochs and blocked-write counts;
- raw latch capabilities, status, receipts, CRCs, cadence, and counters before
  and after the failure;
- route, RGB565 geometry, source-frame digest, and nonblack statistics;
- both readiness-post receipts;
- UIO command-boundary, retry, malformed-response, and timeout details; and
- selected recovery and result.

Every capsule says `sink_visibility: unobserved` unless paired later with an
external capture or operator report. The software classification stops at the
last proven layer: identity, UIO transport, ownership/route, source frame,
latch protocol, or internal ready.

## Qualification contract

### Actual-RTL proof

The production patch now owns the queue transition function in a VHDL package
compiled from the patched `ascal.vhd`; the GHDL harness exhausts that exact
function and the stopped-clock simulation exercises the integrated protocol.
This is stronger than the retired shadow-only model, but the real-scheduler
inductive proof and sequential-equivalence proof below remain qualification
gates. A shadow model plus a structural text pin is not enough.
The harness covers:

- arbitrary asynchronous clock phase and supported clock ratios;
- `clk_hdmi` stopping at every forward and return handshake phase;
- zero, one, and two completions during a stop;
- Avalon wait and return gaps, including the minimum full-block return;
- completion, acknowledgement, queued forwarding, read issue, copy start, and
  copy retirement on coincident edges;
- reset assertion and per-domain release at every protocol state;
- distinctive ordered data and metadata for both buffers;
- repeated pause/resume and toggle wrap; and
- recovery to sustained nonblack raw and final frames.

Assertions make credit loss, duplication, queue overflow, metadata reorder,
copy-level error, phantom reset pulse, stale pre-reset return, or failure to
make progress fatal. A sequential equivalence check proves legacy and repaired
behavior identical whenever no completion is hidden, and refinement to an ideal
lossless capacity-two transport whenever completions are hidden.

### CDC and physical gates

The candidate is built only through the canonical signoff path with Quartus
Prime Lite 17.0.0 Build 595, seed 2, four processors, and matched stock, pinned
pre-observer, and patched identities.

All of these are hard gates:

- setup slack at least `0.428 ns`;
- hold slack at least `0.200 ns`;
- no more than `0.150 ns` setup or hold degradation from the matched baseline;
- zero total negative slack;
- exactly `158` unconstrained output paths, equal to baseline;
- at most `+150 ALMs` and `+96 fitted registers`;
- exactly one new two-register acknowledgement synchronizer with a calculated
  MTBF, and no extra or duplicated synchronizer;
- combined added-crossing MTBF at least `10^12` device-hours at worst reported
  conditions;
- exactly one nonempty reviewed acknowledgement net-delay bound with
  nonnegative slack;
- unchanged RAM, DSP, PLL, latch, route, pixel, reset, and warning identities;
  and
- no observer hierarchy or new UIO response cone.

No false path, multicycle exception, seed sweep, LogicLock, placement directive,
legacy timing edit, warning waiver, or threshold relaxation may manufacture a
pass. An unrelated worst path still rejects the RBF: qualification applies to
the whole physical implementation.

### Exact-RBF hardware matrix

Deterministic proof is followed by a sink-visible matrix using the exact
production tuple. Evidence from an instrumented sibling RBF cannot substitute.

The stress corpus uses a known nonblack, frame-identified, band-sensitive RGB565
pattern and covers:

- cold boot;
- active launcher restart without RBF reload;
- Arcade and ordinary-core return;
- crash respawn;
- every supported mode apply, confirm, cancel, and rollback path;
- repeated PLL reconfiguration; and
- cold-start and thermally settled operation.

Run at least `300,000` total transitions across at least three representative
MiSTer boards and two HDMI sink/capture chipsets, with at least `100,000` per
board. Arcade-to-MiSTer-MagiK return is the dominant cell. Every supported mode
and transition class has a predeclared nontrivial count and is reported
separately; a failing cell is never pooled away.

Zero failures in 300,000 trials gives a one-sided 95% upper defect probability
of approximately `1.0e-5` per transition. This supplements the deterministic
proof and is not a substitute for it.

Capture and classify every emitted physical frame at the output cadence with an
uncompressed HDMI analyzer or capture path. The repository's 30 fps USB Video
path remains useful supporting evidence, but it cannot prove that every 60 Hz
frame is intact. After the first sustained correct MiSTer MagiK frame or
`LauncherActive`, any all-zero active frame, stale frame identifier, vertical
band, partial frame, corruption, OSD/terminal flash, or unexpected black frame
rejects the exact candidate. Intentional pre-ready bootstrap black remains
allowed only inside its existing bounded transition.

The exact tuple must then pass:

- the four continuous qualified-black movies in
  [Qualified Black Bootstrap](bootstrap-black-qualification.md);
- the full HDMI and CRT display matrix with paired authoritative source and
  sink evidence;
- the identity-locked six-hour, 1,000,000-frame latch-v5 gate in
  [FPGA latch-v5 production requirements](fpga-latch-release.md); and
- the unchanged seven-day canary.

Any source, Main, runtime, module, RBF, manifest, build input, or identity change
invalidates all physical evidence.

## Stop and rollback policy

The candidate is rejected on any formal counterexample, queue overflow, missing
or duplicate completion, metadata mismatch, reset anomaly, canonical signoff
failure, skipped hardware cell, identity mismatch, interrupted soak, or single
post-ready black/corrupt/banded/stale physical frame. A path is not excused
because it is outside the repair.

The previous qualified platform and stock route remain recoverable throughout
Dev installation, qualification, canary, and rollout. Internal activation or
readiness failure uses the existing transactional rollback. An attended
external classifier failure triggers immediate typed rollback. A field visual
failure retains the bounded incident capsule and a one-step stock recovery
path; it never depends on the candidate RBF to diagnose or recover itself.

The Gray candidate, reset-only recovery, broad observer, timing waivers, and
placement experiments are not fallback steps. If this queued handshake fails
its predeclared proof or physical gates, stop and return to architecture review.
