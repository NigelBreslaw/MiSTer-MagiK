# Scaler copy-tail repair

Status: local proof candidate; fixed-seed Apple signoff and device validation
remain separate gates.

The disposable schema-6 `scaler-copy-retirement-v1` diagnostic is retired.
Its first genuine persistent-black result isolated a production copy-FSM
deadlock. The repair candidate implements no FPGA diagnostic responder:
commands `0x60` through `0x67` are unsupported, while latch protocol `5` and
capabilities `0x03ff` remain unchanged. The generated schema-6 host decoder is
retained only so already-installed rollback experiments can still be read.

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

The local candidate requires:

- exhaustive GHDL checks for every active-gate input, every supported format,
  all 16 phases, normal non-last retirement, no early last-block retirement,
  and bounded last-tail retirement;
- exact-source formal safety showing every active tail shifts, no tail edge
  creates pixel validity, and tail age remains below 18 output-clock steps,
  plus a retirement cover witness;
- unchanged completion queue BMC, cover, and induction proofs;
- structural proof that no schema-6 observer or responder remains and that
  only the exact `sCOPY` tail sites changed;
- Icarus proof that `0x60` through `0x67` remain unsupported and every
  latch-v5 command remains unchanged; and
- exact two-chain completion CDC/net-delay/MTBF gates, with no diagnostic CDC.

No new register, RAM, DSP, or PLL is expected. The helper logic adds one
registered-state term to the existing copy shift enable and a tail-only clear
of the existing pixel-valid register. Fixed-seed Apple signoff must still prove
commercial setup/hold, resource, warning, CDC, and hard-block gates before any
installation. Device validation must begin by preserving the current incident;
this document authorizes no recovery or device action.
