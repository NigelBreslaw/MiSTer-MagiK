# AGENTS.md - FPGA development and qualification

Root `AGENTS.md` and `mister/AGENTS.md` apply. This file records the shortest
commercial-quality FPGA workflow established while repairing the scaler return
black-screen failure. It is operational guidance, not a waiver of the root
safety, delivery, or release rules.

## Core rule

Prove cheap properties before spending hours in Quartus, then build one frozen
candidate and preserve its exact identity through formal proof, timing signoff,
device smoke, and physical-output qualification.

Do not use CI as the first compiler. A changed FPGA candidate must first pass
the repository simulations and exact-source proof locally, then complete the
canonical Apple-container RBF signoff on this Mac. CI is the production rebuild
and packaging gate after those local results pass.

## Supported local tools

The known-working native tools on the development Mac as of 2026-08-14 are:

- `/opt/homebrew/bin/ghdl`: GHDL 6.0.0, LLVM backend, native arm64. It compiles
  the patched production VHDL and synthesizes the narrow proof DUT that shares
  the production transition functions.
- `/opt/homebrew/bin/yosys`: Yosys 0.68. It runs the repository-owned bounded
  and temporal-induction proof after GHDL synthesis.
- `/opt/homebrew/bin/iverilog` and `/opt/homebrew/bin/vvp`: Icarus Verilog 13.0.
  They run the SystemVerilog protocol and exact-topology simulations.
- Apple `container`: runs pinned Quartus Prime Lite 17.0 Build 595. Quartus is
  not installed or invoked as a native Homebrew program.

Their Homebrew packages are `ghdl` 6.0.0 as a cask, `yosys` 0.68, and
`icarus-verilog` 13.0. Prefer those tracked packages when repairing the local
toolchain; do not silently change versions in the middle of a candidate proof.

Verify versions before relying on a previous setup. GHDL is a command-line
tool installed by the Homebrew cask, not a macOS GUI application and not part
of the Apple Quartus container. If Gatekeeper blocks a freshly installed GHDL
binary, repair or approve the Homebrew cask installation and re-run
`ghdl --version`; do not replace it with an untracked binary or weaken the
proof gate.

Use the repository interfaces to drive these tools. Do not hand-assemble a
different GHDL/Yosys proof command and call it equivalent to the checked-in
formal runner.

## Apple-container Quartus and cache

The only local synthesis entrypoints are:

```bash
QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup
scripts/agent fpga signoff
```

Never invoke Quartus, its installer, or the underlying FPGA build script
directly.

The default reusable root is:

```text
build/fpga-local-apple/
```

In the canonical checkout on this Mac that is
`/Users/nigelb/slint/mister-slint/build/fpga-local-apple/`.

Set `MISTER_FPGA_LOCAL_ROOT` to a stable absolute directory when separate Git
worktrees or agents need to share the installation and completed synthesis.
The important layout is:

```text
$MISTER_FPGA_LOCAL_ROOT/
  quartus-lite-17.0/             pinned installer and installed runtime
  sources/main/                  generated checkout of local main
  sources/pre-observer/          generated pinned baseline checkout
  signoff/stock/                 completed stock synthesis
  signoff/pre-observer/          completed matched baseline synthesis
  signoff/patched/               completed candidate synthesis and RBF
  signoff/quartus-delta-signoff.tsv
  signoff/.stock.building/       incomplete staging, never a cache hit
  signoff/.pre-observer.building/
  signoff/.patched.building/
```

Each completed variant is validated against `local-signoff-input-v5.txt` and
its RBF, metadata, build log, and reports. The cache identity includes the
synthesis inputs, pinned Menu and baseline revisions, Quartus seed, canonical
build date, and preparation-script identity. A patch-only RTL change should
normally rebuild only `patched`; stock and baseline remain reusable when their
identities match. `--rebuild` intentionally discards those savings and must not
be used speculatively.

Interrupted work stays in a hidden `.VARIANT.building` directory and cannot
replace a completed variant. Do not mistake that staging directory for a cache
hit, edit cached reports, or copy one variant's evidence onto another.

`scripts/agent fpga signoff` builds `refs/heads/main^{commit}`, not an arbitrary
worktree `HEAD`. Commit the candidate and advance local `main` before starting
the expensive build. Record the resulting root commit, patched-source SHA-256,
RBF SHA-256, metadata SHA-256, and delta-report SHA-256 together.

## Efficient development sequence

Use this order and stop at the first real failure:

1. Define the causal failure and invariants. Separate the known RTL cause from
   symptoms that still require physical qualification, such as vertical bands.
2. Freeze hard gates and the candidate interface before implementation. Do not
   add diagnostic opcodes or unrelated pixel/output logic during timing closure.
3. Apply and structurally check the production patch. Compile the actual VHDL,
   not only a convenient replica.
4. Run the exact-topology GHDL and Icarus simulations. Exercise reset, stopped
   clocks, simultaneous events, queue limits, and wraparound before formal work.
5. Run exact-source bounded proof and required non-vacuity covers. These catch
   reachable safety defects and broken or unreachable test guidance quickly.
6. Run temporal induction. Production-derived type, pipeline, and coherence
   invariants may strengthen the proof; they must be assertions, never free
   assumptions used to hide a counterexample.
7. Freeze the exact integrated commit. Start formal signoff and the cached
   Quartus build in parallel only now.
8. Reject any real timing, area, CDC, reset, or proof failure. Fix the smallest
   causal cone, repeat the cheap gates, then rebuild only the invalidated
   Quartus variant.
9. Install a locally signed-off RBF only through the attended, rollback-capable
   Dev transaction in root `AGENTS.md`. Never copy or activate it manually.
10. Use output-rate physical capture for device smoke and qualification. A
    framebuffer, protocol acknowledgement, or 30 fps USB movie does not prove
    every emitted HDMI/CRT frame was visible and correct.

Start with `scripts/agent plan` to see the repository assurance selected by the
current diff. Let pre-commit, pre-push, and CI run their owning checks; do not
duplicate those boundaries with an improvised command list. For focused proof
debugging, use `scripts/checks/check-fpga-latch-integration.py --simulate` on
the exact pinned Menu checkout for structural binding plus the GHDL and Icarus
simulations. Run the separate exact proof with
`python3 scripts/checks/check-fpga-scaler-completion-formal.py PINNED_MENU_DIR`;
that runner owns BMC, required covers, and temporal induction. Preserve solver
logs, reports, VCDs, and exact hashes for any claimed failure or pass.

## Formal-model rules learned from the scaler repair

- Analyze and compile the patched production `ascal.vhd` and its package,
  synthesize the narrow formal DUT that calls the exact production transition
  functions, and separately require structural binding to the real scheduler
  sites. A mirrored queue model or source-text matcher alone is supporting
  evidence only; do not claim that the full `ascal` architecture was formally
  synthesized by the current runner.
- Build the responder scoreboard from actual accepted Avalon transactions and
  returned beats. Never derive the reference model from the DUT credit counter.
- Keep `waitrequest`, return gaps, clock phase, and clock stops arbitrary unless
  an exact integrated topology proves a narrower contract. For example, the
  SDRAM path permits a return on an issue edge only when older work was already
  outstanding; it cannot create the first response combinationally from a new
  request.
- Model the production reset exactly: asynchronous assertion and synchronous
  release in each clock domain. A clocked approximation can manufacture missed
  reset pulses and waste many solver/review cycles.
- Retained reset accounting must distinguish asserted, accepted, outstanding,
  returned, queued, pulsed, and consumed work. Test reset on acceptance, reset
  while stalled, late old returns, and release before acceptance.
- Drive covers from events such as acceptance, return, reset, VS, and clock
  edges. Fixed cycle-number scripts become brittle when reset or pipeline
  semantics improve.
- Require non-vacuity covers for zero, one, and two completions during an HDMI
  clock stop; coincident acknowledgement/completion; old returns during reset;
  VS/drain races; and the first ordered post-drain completion.
- Classify a solver result before editing RTL: a reachable bounded
  counterexample, an unreachable cover guide, an impossible induction start
  state, and a production defect are different findings.
- Strengthen induction with exact VHDL range and synchronizer pipeline
  invariants as one coherent set. Do not chase arbitrary states with unrelated
  RTL changes or assume the property being proved.

## Quartus hard-gate discipline

Declare numerical gates before the first build and keep them fixed. The scaler
repair used Quartus 17.0 Build 595, seed 2, setup slack at least 0.428 ns, hold
slack at least 0.200 ns, zero TNS, no more than 0.150 ns matched-baseline setup
degradation, exactly 158/158 constrained relationships, at most +150 ALMs and
+96 registers, unchanged RAM/DSP/PLL identity, and a combined synchronizer MTBF
of at least `10^12` device-hours.

Validate exact forward and reverse CDC endpoints and net-delay rows, not only
global synchronizer counts. Tool recognition can turn a previously unreported
legacy chain into a reported chain after attributes are added; distinguish a
new physical synchronizer from a newly recognized one without weakening MTBF
or topology checks.

Never rescue a failing candidate with a seed sweep, timing waiver, false path,
LogicLock, fitter-setting change, or an unrelated reset controller. Fitter
placement is global: a small local RTL change can degrade an unrelated SDRAM
path. Trust the matched reports, not the apparent size of the source diff.

## Parallel agents without duplicated work

Three focused lanes are normally enough; use a fourth only when runtime/Main or
physical qualification is changing too:

1. **RTL owner:** owns the production patch, directed simulation, reset and
   protocol semantics, and the final candidate commit.
2. **Proof owner:** owns the independent responder, exact-source binding, BMC,
   covers, induction, and proof artifacts. It reviews but does not silently
   modify the RTL owner's candidate.
3. **Physical-signoff owner:** owns cache identity, Quartus constraints, delta
   checker interpretation, area/timing/CDC evidence, and the one expensive
   build.
4. **Runtime/qualification owner, when needed:** owns activation identity,
   fail-closed recovery, Main integration, and physical evidence contracts.

Every lane uses a separate worktree and reports an exact commit plus relevant
source/artifact hashes. One integration owner controls merge order and declares
the frozen tuple. Do not let several agents run Quartus against private variants
or edit the same worktree.

Reviews must be bounded questions with explicit exit conditions, for example:
"Can a newly accepted read return data on the same edge?", "Can reset accept the
same held request twice?", or "Does this exact report satisfy the predeclared
CDC endpoints?" A reviewer returns PASS or one concrete defect with an exact
trace/source reference. Broad repeated architecture reviews after the interface
is frozen consume time without reducing the next risk.

The efficient parallel boundary is:

- RTL and proof-model construction may proceed together against a written
  interface.
- Proof preflight must bind the final RTL hash before it can pass.
- Full induction and cached Quartus signoff may run concurrently after that
  hash is frozen.
- Runtime/Main and qualification work may proceed in parallel when their ABI is
  already frozen, but must not be used to excuse an unqualified RBF.

## Qualification and delivery boundary

A local signoff pass makes the RBF eligible for a bounded attended Dev install;
it does not make it a production release. CI must rebuild and attest the exact
platform tuple. Commercial release still requires the repository's physical
frame-evidence matrix, stress campaign, long latch gate, and canary.

Fail closed on any black, stale, partial, banded, corrupt, or indefinitely blank
physical frame after MiSTer MagiK is revealed. Preserve the previously qualified
platform for rollback throughout development.
