# Passive FPGA HDMI evidence

## Candidate boundary evidence: schema 24

The diagnostic-only candidate `scaler-memory-boundary-v1` replaces the active
schema-23 observer. It leaves the functional scaler and reset terminator logic
unchanged. It compares scaler-facing acceptance with the actual memory-facing
request after `f2sdram_safe_terminator`, checks address/burst equality at that
boundary, and independently counts physical return obligations. The checked
contract is `mister/platform/fpga/menu-vblank-latch/causal-evidence-protocol.json`.

A retained request can be accepted downstream after reset release while the
scaler-facing request is gated. The production-bound scheduler model previously
cleared this request during reset; the corrected model retains it like the
patched VHDL. Connecting that model to the pinned, actual terminator exposes a
counterexample. The sustained-reset simulation reproduces it and requires the
new observer to capture cause 2. This is a known boundary-accounting failure,
not a passing proof of the production boundary and not yet an explanation of
the physical stripes. The candidate is intentionally an observation experiment;
a functional repair needs its own end-to-end return/retirement proof.

The physical ledger represents three 128-beat obligations, so the third accepted
read is evidence rather than an observer FIFO overflow. A fourth obligation,
malformed burst or orphan return permanently invalidates the ledger instead of
wrapping it. The first source event remains immutable across core resets and
reads; a separate live snapshot continues after that event. Only FPGA
reconfiguration initializes the observer.

Commands `0x68`/`0x69` select first/live evidence, with acknowledgement
`0x4d58`/`0x4d59`. Command `0x6a` reads a completed five-word snapshot, with
acknowledgement `0x4d5a`. Partial and overlong reads cannot clear evidence.
The source snapshot is captured before the event edge. Its output-state word
is a **later** acknowledged snapshot from the output clock domain, never a claim
of simultaneity with the first source event. A 64-source-cycle timeout keeps
source evidence readable when that clock stops. Invalid output data is zero
and explicitly marked invalid. Closed-loop mailbox toggles and held payload
constraints cover selector, source bank, output bank and CRC publication.

The native `fpga-evidence-v1` capability exposes only these fixed read operations,
with bounded lock acquisition. It archives both ACK phases, CRC/schema results,
three first/live pairs, before/after latch state, boot/process identity and
capture intervals. Unsupported legacy layouts remain raw evidence, not invented
zero-valued fields. The incident's schema-23 observer-overflow layout remains
decodable. Capability checks refuse bootstrap, repair and service replacement,
even if the normal repair environment variable is set.

After explicit delivery while healthy, capture an incident with:

```sh
scripts/magik device fpga-evidence --framebuffer --usb-seconds 5
```

This saves diagnostics, authoritative RGB565 bytes/PNG and metadata, a physical
USB movie, another diagnostic read, time bounds and SHA-256 hashes. Failed
stages remain recorded while independent stages continue. The command does not
install or activate an FPGA. Service identity is the running native executable;
installed artifacts and Main status do **not** prove the loaded FPGA hash.

For bounded, foreground monitoring while healthy:

```sh
scripts/magik device fpga-evidence --poll-count 720 --poll-interval 5
```

Each sample is independently archived. Source fingerprints exclude the later
output snapshot. First-seen intervals use the previous valid empty first-record
observation in the same boot; without that observation the lower time bound is
unknown. Fingerprints are not configuration-generation identifiers: an FPGA
reload without an OS reboot must end the monitoring campaign. Polling's effect
on physical output remains a required device qualification measurement.

The evidence deliberately has no trace RAM, FPGA timestamp, full address history
or pixel signature. Address comparison identifies command divergence but cannot
prove the descriptor address itself is correct. A coherent output snapshot can
show credits and scheduler state; it is not a complete event history or a proof
of pixel correctness. If control invariants remain intact during corruption,
raw scaler/pixel-stage localization is still required. These limits avoid
claiming that a compact control observer guarantees every corruption diagnosis.

Cheap checks use `check-fpga-causal-evidence.py` (simulation, all nine fault
predicates, independent CRC, bounded safety, temporal induction, required covers
and a same-tool LUT6 comparison), the exact-source completion checker, the
production bridge simulation, and `check-fpga-terminator-boundary.py --replay-only`.
The latter is a **passing detection regression for a known failing boundary**.
Without `--replay-only`, its boundary proof intentionally fails and preserves
the counterexample; never count that failure as production safety signoff.

The `experimental_scaler_causal-v1` profile retains the prior experimental
numerical gates (0.350 ns setup, 0.200 ns hold, at most 0.30 ns degradation,
224 ALMs/224 registers, zero TNS, no RAM/DSP/PLL growth, and combined custom
MTBF at least 10^12 device-hours). It adds exact observer CDC identities.
The final fitted reports must establish those identities and counts; fixture
reports and structural estimates are not a Quartus pass.
Full combinational payload paths are checked independently of clock exceptions:
all 632 connected register pairs across slow/fast 1100 mV at -40/100 C must be
present and no complete path may exceed 10 ns. Direct control-net constraints
remain separate. The CRC mailbox is preserved against router duplication.

## Functional repair and evidence limits

The retained queued-completion and copy-tail repairs, reset proof obligations,
and exact-artifact physical qualification are owned by the
[scaler repair contract](fpga-scaler-return-recovery.md). A diagnostic signoff
pass does not resolve those obligations or qualify a repair-only sibling RBF.

Superseded observer designs and dated trial decisions remain in Git history;
incident records remain in `history/`. Older layouts may be decoded for rollback
evidence, but are not the current candidate protocol or signoff profile.
Physical range normalization remains essential: comparing video-range and
full-range luma without normalization previously produced a false corruption
classification. That retracted event supports no FPGA boundary conclusion.
