# Passive FPGA HDMI evidence

The wide and staged FPGA video observers are retired from production. Their
field evidence was decisive, but every expanded implementation made the dense
legacy scaler physically sensitive and failed at least one fixed qualification
gate.

The later repair-only two-bit Gray candidate is also not qualified. With every
diagnostic opcode removed it still measured `0.072 ns` setup and `0.190 ns`
hold, below the required `0.428 ns` and `0.200 ns`. It must not be installed or
published.

The repair and qualification policy is documented in
[FPGA scaler return recovery design](fpga-scaler-return-recovery.md). It selects
a queued one-bit request/acknowledgement repair which preserves the legacy
HDMI-side completion cone. Platform-v0.29 kept all observer commands out; the
successor retains the separately qualified schema-10 observer.

The experimental attribution sequence through schema 6 is now retired. Its
last result isolated a production `sCOPY` line-last tail that could stop before
the delayed terminal bit reached `o_last2`, permanently preventing `lev_dec_v`.
The repair-only platform-v0.29 RBF exposes no observer: commands `0x60`
through `0x67` are unsupported. The succeeding diagnostic-enabled platform
candidate adds only the read-only schema-10 `0x67` raw-scaler ordered
signature; latch-v5 and capabilities `0x03ff` remain unchanged. The exact
repair, observer, and proof boundary are in
[Scaler copy-tail repair](fpga-raw-scaler-diagnostic.md). A passing
`experimental_raw_scaler-v1` Quartus result is a valid numbered platform
component and is intentionally installed by ordinary delivery.

`0x67` is diagnostic evidence, not a recovery command. It has no write, clear,
arm, reset, or freeze operation. The host requires three CRC-valid, identical
coherent records before classifying completion backlog, idle credit-accounting
stall, or continuing scheduler progress.

That experiment subsequently reproduced physical black with nine identical
coherent `0x0da3` samples. It ruled out the completion queue and the specified
credit-accounting stall for that occurrence, so the scheduler observer is
retired from the next candidate.

Schema 4 then reproduced persistent physical black with correct framebuffer
content and coherent completed active frames whose raw scaler RGB was exactly
black. That observer is now retired rather than stacked.

Schema 5 then reproduced persistent physical black with full read and copy
levels, an idle completion queue, and continuing copy/line/raw timing whose
data remained zero. That broad pipeline observer is also retired rather than
stacked.

Schema 6 decisively observed the stalled terminal predicate and is not present
in the functional repair candidate. Its generated host decoder remains only
for rollback evidence compatibility.

Schema 7 was a separate disposable experiment for the rare moving-band
corruption, not another black-screen repair. Its four-byte-per-cycle CRC cone
failed fixed-seed timing and resource gates before installation and is retired.

Schema 8 replaces it with one shallow ordered-signature update per qualified
sample at the same direct `ascal` taps and one observer-only isolation stage.
It publishes only the latest signature and advancing sequence through read-only
`0x67`; `0x60`–`0x66`, latch-v5, and capabilities remain unchanged. It has no
final-HDMI observer and continues to report sink visibility as unobserved. See
[the retry design](../history/2026-08-24-raw-scaler-ordered-signature-design.md).

The complete attempt history and retained measurements are in
[FPGA video diagnostics: two attempted designs and their retirement](../history/2026-08-14-fpga-video-diagnostics-design-attempts.md).
