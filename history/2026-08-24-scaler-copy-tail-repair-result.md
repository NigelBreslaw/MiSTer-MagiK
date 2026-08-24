# Scaler copy-tail repair result — 2026-08-24

## Candidate

Design 10 repairs the exact production deadlock established by the schema-6
incident. The `sCOPY` shift gate remains active while registered `o_last` is
pending, and a tail-only edge forces `o_copyv(0)=0`. This drains line-last and
word-phase state to the unchanged terminal predicate without creating a pixel
or line-buffer write. The disposable schema-6 observer is absent and commands
`0x60` through `0x67` are unsupported.

- implementation commit: `518862fe4c49e4be252f532f7a943c5529beebf0`;
- production patch SHA-256:
  `fad82f36ec04b09ef758812fd5a4c3766e096f28a91da55a09574eaf7932b839`;
- RBF SHA-256:
  `b1bfb33d564852856fde1e4cfc4671fe1591fa16ad4398202031f7d8bc25512b`;
- metadata SHA-256:
  `62a7a66eada10e00bf69ec430f9471bcd00f81fb9cf71df8be267487c95dd7b4`;
- signoff report SHA-256:
  `b9bfd9519a734f9e847dacc5c7328fdf6fcf8473a8c070854637674f53872e6a`.

## Local proof and fixed-seed signoff

The exact candidate passed patched-production structural, GHDL, and Icarus
checks; generated protocol checks; all 44 Quartus-checker fixtures; copy-tail
reset-reachable BMC and retirement cover at depth 48; and the unchanged
completion BMC, nine covers, and temporal induction.

Canonical Apple-container signoff used Quartus 17.0.0 Build 595 and seed 2.
The production gates passed:

| Gate | Result |
| --- | ---: |
| setup slack | `0.557 ns` |
| hold slack | `0.212 ns` |
| total negative slack | `0` |
| constrained output relationships | `158` |
| ALM delta from matched baseline | `+87` |
| register delta from matched baseline | `+7` |
| RAM / DSP / PLL delta | `0 / 0 / 0` |

The two completion request/acknowledgement CDC paths were present with a
minimum bounded net-delay slack of `9.039 ns`. Each chain reported an MTBF of
`10^9` years, above the `10^12` device-hour requirement.

## Device campaign

The exact RBF was installed through the attended rollback-capable experimental
transaction. Preflight reported latch protocol `5`, capabilities `0x03ff`,
`repair_transport_ready`, stable MagiK ownership, and no passive observer.

Phase 2 used the fixed direct-Arcade launch/return path and a proven 2,000 ms
in-game dwell. A reboot was completed after every ten clean returns. The first
51 transitions across six boot epochs returned to a physically visible MagiK
launcher without a black screen. Two high-row-edge classifier stops were
manually rejected as false positives only after all sampled physical frames
and the authoritative framebuffer matched exact known-good hashes. One
automation lease cleanup returned transient `EAGAIN`; read-only reconciliation
proved the lease clear and the completed return evidence healthy before the
campaign resumed.

Transition 52, epoch 6 attempt 2, failed the physical gate. The first and third
USB-video stills were exact healthy frames. The middle still was a visibly
line-displaced/noisy frame with a new SHA-256, while the authoritative RGB565
framebuffer remained exact and nonblank.

| Artifact | SHA-256 |
| --- | --- |
| healthy physical frame before | `84068db55c9f625b287b8c37426457d742e6b695a6a7f4050d053348a0a04f38` |
| transient corrupt physical frame | `1982056bc94ecc9a6f24a6e6070d6b675808b2fc13dca419b1602c56622fc2f4` |
| healthy physical frame after | `84068db55c9f625b287b8c37426457d742e6b695a6a7f4050d053348a0a04f38` |
| authoritative framebuffer | `288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250` |
| Phase-2 summary | `8f74ae72048f019d77bd14aabeb86c3fa389cbc24d79ca2a3098c7d0fb35dd96` |

The campaign stopped immediately. No reboot, return, RBF reload, or recovery
followed. A native macOS 30-second preservation movie then delivered 755
healthy frames over 30,160 ms: luma mean remained `54`, and the native
row-discontinuity metric remained `73 permille` in every frame. The corruption
had self-cleared before the movie, matching the previously established
single-frame transient class rather than the persistent moving-band state.

Failure-time and later read-only FPGA evidence both remained coherent
`repair_transport_ready`: owner epoch `1`, zero latch drops/rejects, and a
correct 960x540 RGB565 route. The observer correctly reports
`sink_visibility: "unobserved"`; USB video is the physical authority.

## Decision

The exact black-screen repair remains supported by the captured causal RTL
state and its formal proof, and no black screen occurred in this bounded
51-clean-return observation. That is not enough to claim the black issue
qualified: the previous deadlock reproduced only after 75 valid returns.

More importantly, the candidate is rejected for commercial release because a
corrupted physical frame is unacceptable even when it is transient and appears
to be a separate mechanism. Do not modify or weaken the copy-tail repair based
on this event. Freeze it as the black-deadlock candidate. Further RBF work must
address the independent raw-to-final timing/phase corruption with a separate,
minimal proof boundary and must not disturb the completion queue, latch-v5,
route, reset, or copy-tail logic.

Large evidence remains ignored under:

```text
build/scaler-copy-tail-phase2-epoch6-02/
build/scaler-copy-tail-phase2-epoch6-02-live-30s.mov
build/scaler-copy-tail-phase2-epoch6-02-live-30s-metrics.csv
build/scaler-copy-tail-phase2-epoch6-02-live-diagnostics/
```

The compact integrity record is
[`2026-08-24-scaler-copy-tail-transient-corruption-incident-v1.json`](2026-08-24-scaler-copy-tail-transient-corruption-incident-v1.json).
