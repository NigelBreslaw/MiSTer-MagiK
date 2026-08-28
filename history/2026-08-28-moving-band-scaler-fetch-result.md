# Persistent moving band with no schema-11 fetch epoch

## Preservation status

The persistent moving-band corruption recurred on 2026-08-28 with schema 11
active. No reboot, RBF reload, launcher restart, mode change, recovery, or scene
change was performed. Two independently collected read-only evidence sets were
reconciled by boot, process, ownership, and monotonic latch identities. The
device remains `captured-unrecovered`.

Large evidence remains ignored under:

```text
build/agent-diagnostics/2026-08-28-bad-ui-state/
build/agent-diagnostics/2026-08-28-live-corruption-scaler-fetch-v1/
build/captures/2026-08-28-bad-ui-*
```

The compact integrity record is
[`2026-08-28-moving-band-scaler-fetch-incident-v1.json`](2026-08-28-moving-band-scaler-fetch-incident-v1.json).

## Physical and source evidence

Two native USB Video movies, 30.02 and 15.02 seconds long, show the same dark
raster with a narrow bright vertical band moving horizontally. They contain
749 and 375 ordered 1920x1080 H.264 frames. Their SHA-256 values are:

```text
db29cb4a68eee326a0be266779d2789238816e98937f37923a3c058777c4972e
ddc42cf528f9e15e0eb08d0956df2fd7494d5c8dab5f7d8db36eead8d5572859
```

Typed FPGA-latched source captures are correct 960x540 launcher frames. Three
adjacent captures are byte-identical at SHA-256
`d565500388d2c33cc0bc222ddccfd6c273feef15464f57f42dd1d1231e7192ae`.
Across the longer capture window the generated difference image contains only
the changing clock glyphs. The physical corruption is therefore not present in
the source framebuffer.

## Schema-11 result

Seven passive bundles span device uptime 1,435,390 through 3,238,270 ms in the
same boot. All 21 schema-11 samples are valid transport records with the same
payload:

```text
schema=11 flags=0 sequence=0 signature=0000 crc=6510
```

Thus `capture_valid=false`, every fault flag is clear, and no fetch epoch has
ever been published in the observed boot. This is not a stable fetch signature
and not a changing fetch signature; the frozen host classification correctly
remains `scaler_fetch_ordered_evidence_inconclusive`.

The absence is persistent while production presentation remains active. Main
PID 2850, launcher PID 2863, Main generation 1223159, owner epoch 1, and boot
ID 843 remain unchanged. Latch active sequence, transaction, and route epoch
each advance by 30; active base alternates between scanout slots; posts and
flips match; drops and rejects remain zero. The running launcher is clean build
5535 at revision `75b962c19ecf8162f97ed5bf0134ebf065286560`.

## Interpretation

This result narrows the failure more than an ordinary malformed sample but is
not the exact root cause. The observer's CRC-protected all-zero snapshot can be
created only after `reset_req` is deasserted. It proves that the observer is
readable, yet since its most recent reset it has seen fewer than two completed
accepted-address wrap epochs. With correct new scanout slots continuing to be
latched while the physical output shows moving stale-looking bands, the leading
failure class is absent or nonresumed external scaler-fetch progress.

Schema 11 cannot distinguish among:

- `vbuf_read` never being asserted;
- requests being held behind `vbuf_waitrequest`;
- accepted reads receiving no or incomplete returns;
- completed requests never reaching the expected address wrap;
- a reset/restart transition clearing fetch progress and failing to rearm it.

No claim should yet separate Avalon fabric behavior from ascal scheduling.

## Serious next step

Do not proceed to the address/data split or HSCAL split: both assume advancing
schema-11 epochs, which this incident disproves. Replace schema 11 with one
narrow external-fetch liveness and first-stall recorder. It should retain no
wide data, add no production feedback, and capture:

1. a clock heartbeat and reset assertion/deassertion epoch;
2. saturating request, accepted-request, return-beat, completed-burst, and
   accepted-address-wrap counters;
3. current and maximum waitrequest streak, two-entry observer FIFO depth,
   return phase, and a folded last accepted address;
4. the first bounded window after reset deassert in which the expected next
   transition fails, frozen as `{read, waitrequest, accepted, returned,
   burstcount, fifo depth, return phase, address fold}`.

That single replacement diagnostic gives decisive branches:

- no requests: move one boundary inward to the exact ascal request-scheduler
  enable/state transition;
- requests but no accepts: Avalon waitrequest/fabric side;
- accepts but no complete returns: return/fabric or backing-memory side;
- complete bursts but no wrap: accepted-address scheduling/range generation;
- normal liveness after a reset while output remains corrupt: move to the
  HSCAL/OLBUF split.

The final recorder after that result must expose the first exact skipped,
repeated, stale, misordered, or unsatisfied production transition. Until then,
the current result is a strong fetch-progress lead, not root cause.
