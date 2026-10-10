# Scaler request reset repair

A reset at a completed high request phase could create an uncharged read. The
output reset cleared `o_read_pre` but retained the transmitted `o_read`. The
Avalon receiver also retained its synchronizer stages and registered pulse.
After release, the transition to zero became another request. Its copy retired
with no output read credit, wrapping the two-bit `o_readlev` from zero to three.

The repair adds seven assignments to the existing reset branches:

- Output: `o_read`, `o_readack_sync`, `o_readack_sync2`, `o_readack`.
- Avalon: `avl_read_sync`, `avl_read_sync2`, `avl_read_pulse`.

All seven reset to zero, matching the existing source phases. Core reset still
retains `avl_read_i`, `avl_read_accepted`, `avl_return_credits` and
`avl_return_phase`. Accepted memory transactions remain obligations across reset;
old responses drain without creating work for the new output epoch. The first
repair leaves read-credit arithmetic, copy retirement, the terminator and passive
diagnostics unchanged. The boundary repair below extends it.

## Host validation

Use the pinned Menu checkout and the tool versions in
[FPGA development](fpga-development.md). All commands below are host-only. The
integration checker reads Menu files from the pinned Git object, applies the
current patch, and preserves generated sources, waveforms, logs and hashes.

```sh
python3 scripts/checks/check-fpga-scaler-reset.py "$MENU" \
  --artifacts-dir build/scaler-reset
python3 scripts/checks/check-fpga-scaler-request-formal.py \
  build/scaler-reset/source/sys/ascal.vhd --artifacts-dir build/request-formal
python3 scripts/checks/check-fpga-scaler-completion-formal.py "$MENU" \
  --artifacts-dir build/completion --report build/completion.json
python3 scripts/checks/check-fpga-terminator-boundary.py "$MENU" \
  --scheduler-artifacts build/completion --artifacts-dir build/boundary
python3 scripts/checks/check-fpga-causal-evidence.py --artifacts-dir build/causal
python3 scripts/checks/check-fpga-latch-integration.py "$MENU" --simulate
```

Run each command even when another fails, preserving individual exit statuses.
A failed safety check remains a failure even when its counterexample is a known
regression. The boundary replay now requires conservation and healthy recovery.

The complete-scaler regression uses GHDL's fixed-width netlist for the production
VHDL and the patched pinned `f2sdram_safe_terminator`, with syntax-only normalization for
Icarus. Unspecified synthesized storage receives a one-time zero power-up value;
explicit RTL initial values are preserved. No state is forced after power-up.
The geometry is 960x540 RGB565 scaled to 1920x1080, with production `run=1`.

The memory responder counts physical acceptances and returns independently of
DUT credits. Each acceptance creates 128 response beats. The checker compares
remaining physical words with retained production accounting at every memory
clock, checks that old responses stay in the drain, and verifies output credit
conservation using the real issue toggle and copy FSM retirement. Every successful
case must resume at least three accepted reads and three copies after reset.

The original 72-case matrix includes both idle toggle phases, a no-reset control, requests
and acknowledgements in flight, copy completion and retirement, one and two
accepted reads, partial bursts, return gaps, acceptance during reset, final old
responses after release, stopped clocks, repeated resets, three additional clock
ratios/phases, and reset offsets around pipeline events. The same matrix runs
against a mutation removing exactly the seven assignments. The original idle
high-phase case must fail specifically with an uncharged read.

The request formal check extracts the production sender, synchronizer, pulse and
acknowledgement assignments and their reset values. Its environment may request
again after acknowledgement and may delay acceptance arbitrarily; independent
clock enables allow either domain to stop indefinitely. This compositional proof
checks transport conservation. Its abstract acceptance environment does not
prove the physical terminator or copy retirement. Those paths remain covered by
the complete-RTL matrix and the separate boundary checks.

The existing completion proof checks its exact-source functions, bounded safety,
induction and required covers. Its abstract pending-read counter and guarded
copy-retirement input exclude the original phantom-request path. A pass of that
check alone cannot establish closure of this incident.

## Boundary repair

A wait-stalled request retained by the terminator can be accepted downstream
after core reset release while the scaler-facing request is gated off. The
independent ledger then contains 128 outstanding words while production
accounting contains zero. The complete-scaler `stage-12` case now exercises that
release window directly, alongside the existing formal counterexample and
observer detection regression.

This failure exists with and without the seven assignments. The boundary repair
keeps `avl_read_i` and its address until acceptance, removes reset/FSM eligibility
from the external request and accounting predicate, and prevents new commands
while old returns drain or the terminator owns the bus. The retained acceptance
guard still suppresses duplicate requests. Old acceptance does not acknowledge a
new output request: reset has already returned the scheduler to `sIDLE`.

One functional `vbuf_busy` wire combines `reset_out`, both vbuf reset pipeline
stages and the terminator's existing ownership flag. It feeds the Avalon domain
at the same 100 MHz clock. It blocks new launches and descriptor replacement;
it never withdraws a presented read. Passive diagnostics remain passive.

Drain release requires zero credits, zero return phase, no unaccepted held read
and no boundary ownership. Both the arming and release cycles check that same
predicate; losing quiescence clears the pending release. Write-pointer alignment
uses the qualified release, preserving the existing one-cycle pipeline.

### Interrupted writes

The connected proof also found stale write ownership in the pinned terminator.
Its observer ended a burst when the counter reached the last beat even if that
beat was stalled. The next accepted beat could start a phantom second write.
Reset could then retain that stale write over a real read, causing upstream
read credit without downstream read acceptance.

Four predicate changes repair this accounting without adding state:

- Advance the write observer only on a physical accepted write beat.
- End an observed burst only when its final beat is accepted.
- Include the reset-takeover edge in the terminating count only if that edge
  physically accepted a write beat.
- End a terminating burst only when its final beat is accepted.

The module is shared by all three memory ports, so these handshake corrections
apply to each instance. The new ownership output is connected only for vbuf.
No burst counters, widths, reset stages or memory data paths were added.

### Proof boundaries and deletion audits

The complete-scaler matrix adds 20 combinations of held-read stalls and external
reset lead time, and checks physical request/address/burst stability across reset.
It uses actual production VHDL, the patched terminator and an independent memory
ledger. The request-reset mutation remains, and a second mutation restores only
the reset/FSM eligibility gate on both exported reads and accounting; `stage-12`
must detect its physical ledger mismatch.

The connected bounded proof includes arbitrary waitrequest, return gaps, reset
pulses, output clock stops and an independent external reset source. Legal write
bursts contend with reads and may be interrupted. Assertions compare physical
acceptance and remaining words with the production ledger, check read descriptor
identity, and prohibit a read before an accepted write burst finishes. Required
witnesses exercise reset with a held read and a read after interrupted-write
ownership. The completion and request proofs retain their temporal induction.
The boundary proof is bounded; it does not establish unbounded whole-scaler safety.

Directed boundary simulations cover acceptance before, during and after release,
all 128 old return beats, interrupted 4/128-beat writes, reset after 1/2/127 beats,
final-beat stalls and acceptance while reset is asserted. Complete-scaler cases
must recover at least three accepted reads and retired copies.

Deletion audits were performed after the request repair, after the connected
write counterexample and before final validation. They removed the obsolete
acceptance-eligibility argument and the test model's cancellation behavior, and
reused the existing request, guard, drain pipeline and terminator ownership.
Removing each of the four write corrections independently made the directed
regression fail. The cases exposed 125/128 or 127/128 accepted write beats, a
dropped terminating final beat, or a read that could not resume.

The production repair adds combinational conditions and wiring, with no new
register, queue, counter or FSM. Tests extend the existing harnesses. Fixed-seed
Quartus timing/CDC signoff and physical qualification remain separate gates.

## First repair host result, 10 October 2026

The complete matrix passed 71 of 72 cases with the repair and 43 of 72 with the
seven assignments removed. All 28 newly passing cases failed before the repair;
no passing baseline case regressed. Both versions failed `stage-12` with physical
outstanding words equal to 128 and production words equal to zero.

The request transport passed 64-step bounded safety, temporal induction and its
64-step stopped-clock/high-phase-reset/recovery witness. The existing completion
proof passed its safety, induction and required covers. The production bridge
simulation and passive-observer checks passed. The terminator safety proof failed
and its separate detection regression passed, preserving the known failure.

These results close the reproduced toggle mechanism within the checked contract.
At that stage the overall host safety result was failed because of the
terminator defect.
No device operation or Quartus build was performed.

## Integrated repair host result, 10 October 2026

The candidate passed all 92 complete-scaler cases. Removing the seven request
reset assignments passed 64/92; restoring reset/FSM eligibility to both the
exported request and accounting reproduced `stage-12` with 128 physical words
and zero production words. The 19 directed boundary cases passed, including
128-beat writes and old read returns. Each of the four independent write-predicate
removals failed its directed regression.

Request transport, completion accounting and observer safety passed bounded
checks, their existing temporal induction and all required witnesses. The
connected boundary passed 64 steps with both reset-with-read and
read-after-interrupted-write witnesses. The production integration simulations,
passive observer checks, Python quality checks and pre-commit gate passed.

The larger boundary solver needed a 600-second budget after the original
180-second attempt timed out. Netlist simplification retained all assertions,
input freedom and the 64-step depth. An additional whole-boundary induction
experiment did not converge within 16 steps; it is not an unbounded safety pass.

The structural Yosys proxy changed the scaler from 4,909 to 4,925 generic cells.
Its 1,191 register cells and 13 memories were unchanged. The 128-bit terminator
remained at 82 generic cells and 13 register cells. These are unmapped host
counts, not FPGA area or timing. The increased scaler count is not a structural
QoR pass under the playbook's synthesis gate. No Quartus build, branch movement
to local `main`, deployment or device operation was performed.

Final evidence is under `build/terminator-proof/` in the implementation worktree:
`summary.json` selects the final reports, and `SHA256SUMS` binds the reports,
logs, waveforms and production inputs. Earlier directories retain exploratory
failures and timeouts; they are not substituted for final passing runs.

## Fixed-seed refinement, 11 October 2026

The first Quartus candidate, `2b30553e4a43`, failed the pinned experimental
certification profile: setup was 0.281 ns, hold was 0.197 ns, and the ALM delta
was 257. The limits are 0.350 ns, 0.200 ns and 224 ALMs. CDC, payload delay,
combined MTBF and zero TNS passed. Two omitted terminator `busy` ports also
changed the connectivity-warning inventory. This candidate was not deployed.

The first refinement selects each line offset before addition. It removes two
wide adders from the address calculation without changing command eligibility,
held descriptors, acceptance accounting or drain release. Write operands remain
sliced before addition; reads still add all 32 bits before slicing. A
combinational SAT comparison of the extracted old and new VHDL assignments
proves identical read and write addresses for every operand and line selection
at the deployed widths. RAM1 and RAM2 explicitly leave the unused ownership
output open; vbuf retains its functional ownership connection.

The full scaler proxy falls from 4,925 to 4,923 generic cells, with 1,191 register
cells and 13 memories unchanged. The isolated address logic falls from 866 to
465 technology-mapped cells. It adds four one-bit muxes and raises the maximum
line-select fanout from 28 to 32. These host counts establish the deletion;
they do not predict fitted timing or area. Evidence is under
`build/refinement-1/`. The fixed seed, profile, constraints, fitter settings and
certification thresholds remain unchanged.

The refinement passes all 92 complete-scaler cases, all 19 boundary simulations,
the 64-step connected boundary proof and both witnesses, request and completion
induction, all completion/liveness witnesses, observer checks and integration
simulations. Each independent removal of a write-accounting predicate still
fails. `build/refinement-1/host-summary.json` and `SHA256SUMS` bind 562 evidence
files, including an applied-source equality check for a final patch-context
normalization. Device state was untouched throughout these host checks.


## SPDIF path identity approval, 11 October 2026

Candidate `cde7137a6` passed the seed-2 timing, area, warning and CDC checks:
setup 0.609 ns, hold 0.250 ns, 213 additional ALMs and 201 additional registers.
The remaining failure was the exact unconstrained-output inventory. Its two
extra LED paths matched the existing exception, but one SPDIF path used a
fitter-created copy of its original register. Both complete path tables and the
fitter duplication record identify the same source, destination and clock.

The approved checker change normalizes that exact substitution only after
checking its fitter evidence in both analyses. It retains the pinned logical
inventory hash and all numerical gates. The checker passes 64 tests, including
missing or ambiguous duplication evidence, incorrect tuples, additional paths
and inconsistent setup/hold substitutions. No RTL, constraint or fitter input
changes accompany this approval. Review evidence is in `build/refinement-2/`;
its proposal evaluation did not issue a build certificate or deploy anything.
