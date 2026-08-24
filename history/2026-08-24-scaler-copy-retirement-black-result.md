# Scaler copy-retirement Phase 2 black result — 2026-08-24

## Result

The locally signed schema-6 diagnostic RBF reproduced the genuine persistent
MagiK black-screen failure on boot epoch 8, attempt 6, after 75 valid clean
returns. The campaign stopped immediately. The device remains unrecovered: no
subsequent transition, launcher restart, RBF reload, or reboot has occurred.

The 1920×1080 native USB-video still is uniform video-level black with minimum,
maximum, and mean luma all `16`. Its SHA-256 is
`dc4ee4f1eb9ede8c4b031b29fc8ba97d72068a296fb716cec2de087b6b25255e`,
byte-identical to the prior persistent-black incidents. The authoritative
960×540 RGB565 framebuffer is complete and varied with SHA-256
`288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250`,
again byte-identical to the correct Arcade return. A 30-second native movie was
captured before recovery and remains ignored under the artifact root.

## Decisive FPGA evidence

Failure-time and later live snapshots both classify the state as
`scaler_copy_terminal_condition_stall`. Each contains three identical,
CRC-valid, coherent completed-frame records with stable MagiK ownership and
`LauncherActive`:

- flags `0x15e1` (`5601`) and state `0x83ea` (`33770`);
- copy FSM continuously in `sCOPY`;
- `o_readlev=2` and `o_copylev=2`;
- `o_adturn=1` and active copy writes;
- front metadata `{prim=1,last=1,bank=1,offset=0}`;
- active shifting, next-word phase, delayed line-last, and address wrap each
  observed during every completed frame;
- bank-terminal term absent because the front entry is the last block;
- combined terminal condition absent, therefore `lev_dec` absent;
- copied DPRAM data remained zero throughout the observed frames.

Latch-v5 continued posting with zero drops and zero rejects. The observer
correctly reports `sink_visibility: "unobserved"`; the native capture is the
physical authority.

## Root-cause localization

The production terminal branch is:

```text
adturn && shift_onext(acpt + 1) &&
((ad mod BLEN == 0 && !front_last) || last2)
```

and that same branch sets `lev_dec`. For a last block, the bank-boundary
alternative is intentionally unavailable, so retirement depends on delayed
`last2` aligning with the next-word phase while the copy branch is active.
This incident proves every component progresses but the combined predicate
never occurs. Horizontal-copy activity then ceases without a retirement edge,
leaving both retained levels full, inhibiting further reads, and repeatedly
presenting zero data.

The smallest repair candidate must let the existing copy FSM drain the delayed
last-line/word phase until that exact terminal branch fires. It must not change
the completion queue, latch-v5, route, reset, PLL, mux, framebuffer addressing,
or pixel format. Exact-source proof must cover every starting word phase,
normal non-last bank retirement, final-block retirement, one-and-only-one
`lev_dec`, bounded completion, and unchanged copied pixel ordering.

## Campaign note

At epoch 4 attempt 4, a byte-identical repeat of the already documented
single-sampled transient physical corruption was fully preserved. It
self-cleared without recovery: healthy still `5e106e…`, two corrupt stills
`9c55f0…`, then the same healthy still again. Schema 6 remained
`scaler_copy_retirement_active` throughout, so that duplicate did not count as
a valid return and does not implicate copy retirement.

## Frozen identity

- host and FPGA builder commit:
  `0ada2bfe6e44bbdeaf314b884a8add1d8b984e4d`;
- Menu source commit:
  `3c3634c0105d78f27aeba66b38966c50dbc42c9b`;
- FPGA patch SHA-256:
  `d732a82008d6abd9868dc7e46f829cb8c8d77d46bebc65efa9e818965348aa4c`;
- RBF SHA-256:
  `ecd075221d3578acdd9d9b182c12e52b1068caa70725de2d08c38bea8c7d4fe0`;
- metadata SHA-256:
  `962c30e7c9f5860231b6f44f8f2d8c375f00e99460043b26424c6bf25236649f`;
- signoff report SHA-256:
  `fe8331896e60a8f40571a94d84ace17f8f730f9c753bf35d823ffc229dbbbe89`;
- local signoff: setup `0.564 ns`, hold `0.241 ns`, zero TNS;
- device agent version/protocol: `29` / `2`;
- runtime launcher: `0.2.5219`, build `5219`, source
  `aa954e639ee3461c90f0420f9eed56ec00e6b637`;
- Main PID/generation: `2843` / `239691`;
- launcher PID: `2863`;
- owner epoch: `1`.

Compact evidence is retained in
[`2026-08-24-scaler-copy-retirement-black-incident-v1.json`](2026-08-24-scaler-copy-retirement-black-incident-v1.json).
Large media remains ignored under
`build/scaler-copy-retirement-phase2-epoch8-06/`.
