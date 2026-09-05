# Scaler copy-tail repair and ordered-signature experiment

Status: the functional repair remains unchanged. Schema 7 and both schema-8
observers failed unchanged local signoff gates and were never installed. The
schema-10 diagnostic platform candidate retains the 16-bit ordered signature,
generates its response sequence after coherent capture, and samples the native
RGB565 color information. It passed fixed-seed Apple signoff and is eligible
for a numbered platform release under the checked-in
`experimental_raw_scaler-v1` signoff profile. Phase 2 return 45 was
initially classified as physical corruption, but native review proved that all
three stills were normal and the following 749-frame movie was healthy. The
false positive came from comparing video-range and full-range luma without
normalization; it supports no FPGA boundary conclusion. The corrected detector
then completed 75/75 valid physical returns across 15 bounded reboot epochs
without reproducing a black screen or moving/full-raster corruption. Schema 10
is the platform publication target so ordinary delivery retains the observer
for the next genuine physical failure. No post-OSD probe was justified by the
completed rerun. The machine-readable [retracted incident](../history/2026-08-25-raw-scaler-signature-v3-corruption-incident-v1.json)
retains the exact artifact identities. Rejected design narratives remain in Git
history.

The disposable schema-6 `scaler-copy-retirement-v1` diagnostic is retired.
Its first genuine persistent-black result isolated a production copy-FSM
deadlock. The new experiment adds only read-only command `0x67`, schema 10,
`raw-scaler-ordered-signature-v3`; commands `0x60` through `0x66` remain
unsupported, while latch protocol `5` and capabilities `0x03ff` remain
unchanged.

## Ordered-signature contract

The schema-10 architecture identity is `raw-scaler-ordered-signature-v3`.
Read-only command `0x67` uses magic `0x4d57` and five response words: schema,
flags, wrapping capture sequence, 16-bit ordered signature, and
CRC-16/CCITT-FALSE. The response remains immutable during a read.

The sole production taps are scaler CE and direct `ascal` RGB, DE, HS and VS.
An observer-only HDMI register stage isolates them; the retained implementation
adds a CE-qualified observer pipeline stage and stores native RGB565 information.
Observer signals must not feed scaler, completion, copy-tail, framebuffer,
latch, route, reset, OSD, mux, PLL or output logic.

Each qualified pixel or line-end contributes an ordered update. The token's
32-bit RGB/boundary information is folded by XORing its halves before a single
16-bit reflected Galois step with polynomial `0xa001`. Empty frames do not
publish. A two-stage generation synchronizer and settle cycle coherently capture
the published signature in the system domain. The wrapping response sequence
advances there only for a newly captured publication; it is not an additional
source-domain sequence bus. Exact CDC source identity must remain qualified.

The host requires three valid records with strictly advancing sequences. Equal
16-bit signatures are limited diagnostic evidence, not proof of pixel equality.
A downstream interpretation requires all three to match a healthy static scene
from the exact same candidate, plus independently detected moving physical
corruption during capture. Changing signatures require independently byte-stable
source proof before supporting an at-or-before-`ascal` origin. Ambiguous scene,
source, sequence, transport or physical evidence remains inconclusive. The FPGA
never proves sink visibility.

Normalize video-range and full-range luma before temporal comparisons: an
unnormalized range change previously produced a false corruption classification.
That event established no new FPGA root cause and justified no post-OSD probe.

Simulation must cover ordered pixels/lines, empty frames, reset, sequence wrap,
immutable and partial reads, CRC and unsupported commands using an independent
signature model. Existing completion/copy-tail proofs, exact observer isolation
and the checked-in `experimental_raw_scaler-v1` signoff profile remain required.
Use the current profile for timing, resources and CDC limits; historical fits
are not permission to relax a gate.

## Decisive result

After 75 valid returns, the installed schema-6 RBF reproduced a uniform
physical MagiK black screen with the authoritative framebuffer still correct.
Three coherent records were identical: flags `0x15e1`, state `0x83ea`, copy
state `sCOPY`, `readlev=2`, `copylev=2`, `o_adturn=1`, front `prim=1`, front
`last=1`, front bank `1`, offset `0`, and `o_copyv(0)=1`.

The frame had copy shifts, next-word phases, line-last activity, and address
wrap, but no bank-terminal event, exact terminal event, `lev_dec_v`, or
nonzero copied word. This rules out a lost decrement after a successful
terminal branch. It identifies a last-block terminal-condition stall.

## Exact defect

The legacy `sCOPY` word/last pipeline advanced only while:

```text
hcarry_v or o_dshi > 0
```

The final horizontal-carry edge registers `o_last = 1`. On the following edge
`hcarry_v` is already false and `o_dshi` is zero, so the branch stops before
`o_last` can pass through `o_last1` to `o_last2`. For a front block with
`last=1`, the alternative bank terminal is deliberately false. The copy FSM
therefore never reaches its existing terminal branch, never asserts
`lev_dec_v`, and permanently holds both two-entry scheduler levels full.

## Minimal repair

The shift gate becomes:

```text
hcarry_v or o_dshi > 0 or o_last = 1
```

The added `o_last` term keeps the existing word phase and two-register
line-last pipeline moving until the unchanged terminal semantics can retire
the last block. On a tail-only edge (`not hcarry_v`, `o_dshi=0`, `o_last=1`),
`o_copyv(0)` is forced low. Tail edges therefore retire only phase and
line-last state; they create no new pixel-valid sample or line-buffer write.
Already-valid delayed samples from the final real horizontal carry continue
through the existing pipeline normally.

Three exact helper functions are compiled from patched production `ascal.vhd`
and shared by synthesis and proof:

- `copy_shift_active` — legacy shift cases plus only the registered line-last
  tail;
- `copy_shift_onext` — the unmodified 8/16/24/32-bpp word-phase truth table;
- `copy_terminal_ready` — the unmodified terminal predicate.

Normal non-last blocks retain the legacy gate because `o_last=0`. The common
reset and each first-line initialization explicitly clear `o_last`, `o_last1`,
and `o_last2`. No scheduler/completion transport, framebuffer, latch, route,
reset controller, PLL, mux, or output cone is otherwise changed.

## Proof and qualification boundary

The rare moving-band movie leaves the authoritative RGB565 framebuffer
correct while completion, copy-tail, latch, and route evidence remains
coherent. Schema 10 fingerprints ordered active RGB565 and line boundaries at the
direct `ascal` output through an explicit isolation register. It publishes one
shallow 16-bit ordered signature and sequence; the host requires three
strictly advancing frames. It does not tap a final output cone and cannot infer
sink visibility.

Stable raw evidence paired with moving physical corruption supports a fault
downstream of direct `ascal`; an ordered-signature change paired with an
independently byte-stable source support an at-or-before-`ascal` fault. Without
that independent proof, changing evidence is inconclusive because a scene
transition has the same local signature.

The source CDC bundle contains only the 16-bit signature. The destination
advances the 16-bit response sequence on each coherent capture.
One destination valid bit reconstructs the unchanged flags word after coherent
capture, including when the 16-bit sequence wraps to zero. This retains the
settle cycle and immutable response while avoiding duplicate 16-bit flag words.
The preserved one-bit `source_generation` register is the exact constrained
CDC source for `generation_meta`. Its synthesis identity forbids replication,
and the signoff parser rejects wildcarded or fitter-created duplicate sources.
The existing two-stage synchronizer and destination settle cycle keep the
published signature stable before coherent capture.

The direct-tap isolation registers feed one further observer-only HDMI-domain
pipeline stage before the signature recurrence. This keeps the sole direct
consumers physically separable from the signature cone; it adds one sampling
cycle without changing frame order, schema, or evidence meaning. Neither stage
has production fanout. The second stage holds its data on unqualified cycles
and captures only when the isolated scaler CE is asserted, so it preserves
every ordered sample without unnecessary observer switching.

The local candidate requires:

- exhaustive GHDL checks for every active-gate input, every supported format,
  all 16 phases, normal non-last retirement, no early last-block retirement,
  and bounded last-tail retirement;
- exact-source formal safety showing every active tail shifts, no tail edge
  creates pixel validity, and tail age remains below 18 output-clock steps,
  plus a retirement cover witness;
- unchanged completion queue BMC, cover, and induction proofs;
- structural proof that schema 10 has only the exact direct-ascal taps, an
  observer-only isolation stage, response-only fanout, and no protected-cone
  changes;
- Icarus proof of the one-step ordered signature, line-order changes, empty
  frames, sequence advance/wrap, immutable and partial reads, reset, unsupported `0x60`
  through `0x66`, and every unchanged latch-v5 command; and
- exact completion CDC/net-delay/MTBF gates plus the one bounded
  source-generation synchronizer route used by the stable observer bundle.

The functional repair itself adds no new register, RAM, DSP, or PLL. The helper logic adds one
registered-state term to the existing copy shift enable and a tail-only clear
of the existing pixel-valid register. Fixed-seed Apple signoff must still prove
commercial setup/hold, resource, warning, CDC, and hard-block gates before any
installation. The disposable observer must remain within the checked-in
experimental profile ceilings of 224 registers and 224 ALMs without RAM, DSP,
or PLL changes; fixed-seed synthesis remains the authority. Device validation must begin by preserving
the current incident; this document authorizes no recovery or device action.
