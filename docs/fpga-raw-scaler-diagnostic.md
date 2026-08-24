# Scaler copy-tail repair and ordered-signature experiment

Status: the functional repair remains unchanged. Schema 7 and both schema-8
observers failed unchanged local signoff gates and were never installed. The
current disposable schema-10 candidate retains the 16-bit ordered signature,
generates its response sequence after coherent capture, and samples the native
RGB565 color information. Fixed-seed Apple signoff and device validation remain
separate gates. The rejected fits and current reduction are recorded in
[the storage-reduction receipt](../history/2026-08-25-raw-scaler-signature-storage-reduction.md),
[the schema-9 design](../history/2026-08-25-raw-scaler-signature-v2-design.md),
and [the schema-10 design](../history/2026-08-25-raw-scaler-signature-v3-design.md).

The disposable schema-6 `scaler-copy-retirement-v1` diagnostic is retired.
Its first genuine persistent-black result isolated a production copy-FSM
deadlock. The new experiment adds only read-only command `0x67`, schema 10,
`raw-scaler-ordered-signature-v3`; commands `0x60` through `0x66` remain
unsupported, while latch protocol `5` and capabilities `0x03ff` remain
unchanged. Its exact taps, byte encoding, ABI, interpretation boundary, and
cost are frozen in
[the dated ordered-signature design](../history/2026-08-24-raw-scaler-ordered-signature-design.md).

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
One dedicated `generation_launch` register follows `source_generation` by one
HDMI edge, after the signature is stable, and has only `generation_meta` as its
data fanout. The exact constrained CDC identity is
`generation_launch -> generation_meta`; wildcarded fitter duplicates are not
accepted.

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
experimental profile ceilings of 224 registers and 208 ALMs without RAM, DSP,
or PLL changes; fixed-seed synthesis remains the authority. Device validation must begin by preserving
the current incident; this document authorizes no recovery or device action.
