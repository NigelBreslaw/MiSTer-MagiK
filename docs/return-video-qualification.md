# Return Video Qualification

`mister-magik-return-frame-evidence-v1` is the canonical evidence contract for
qualifying visible HDMI output after a return to MiSTer MagiK. It is deliberately
separate from `/dev/fb0`, latch counters, launcher health, and FPGA telemetry:
those signals can explain a fault, but they do not prove that a complete frame
reached a physical HDMI or CRT sink.

## Evidence boundary

Every frame-evidence document is strict JSON. Unknown fields are rejected. The
document must identify the exact `platform-v3.manifest` candidate, board, sink
unit, sink chipset, capture, transition class, and video mode. It must also
declare:

- attended, physical HDMI RX or CRT ADC/analyzer capture;
- a frame-complete capture path operating at or above the expected refresh;
- a contiguous first-to-last physical-frame sequence with no missing frames;
- nonblack source content for every classified frame;
- zero black, stale, partial, banded, and corrupt classifications; and
- SHA-256 identities for the capture artifact and classifier.

Each evidence document names a sibling per-frame NDJSON classifier report and
its SHA-256. Every report line contains the physical-frame sequence, source
nonblack result, and classification. The verifier ingests the complete report,
checks its hash and contiguous sequence, and derives counts that must exactly
match the evidence summary. A zero-count summary without that input file cannot
be recorded into a board certificate.

```json
{"schema":"mister-magik-physical-frame-classification-v1","frame_sequence":1,"source_nonblack":true,"classification":"correct"}
```

The verifier does not capture video and does not infer sink visibility from a
framebuffer. A lab producer must generate both inputs from a frame-complete
physical capture/classifier. A 30 fps USB capture cannot qualify a 60 Hz output
and is rejected by the rate check; it remains useful supplementary evidence.

Validate one producer document before aggregation:

```bash
scripts/agent release frame-evidence verify PATH
```

## Per-board certificates

An attended operator binds one or more verified frame-evidence documents to the
strict installed candidate manifest:

```bash
scripts/agent release return-qualification record-board \
  --candidate PATH/TO/platform-v3.manifest \
  --layout public \
  --frame-evidence PATH... \
  --output PATH/TO/board-certificate.json \
  --attended
```

All evidence in a board certificate must name the same board and exact candidate.
Canonical SHA-256 integrity digests bind each embedded evidence value so that
editing an embedded classification or identity invalidates the certificate.
They are not digital signatures; cryptographic signer trust is not asserted by
this format.

## Aggregate release certificate

The host-side aggregate is also bound to the exact strict candidate manifest:

```bash
scripts/agent release return-qualification aggregate \
  --candidate PATH/TO/platform-v3.manifest \
  --layout public \
  --board-evidence PATH... \
  --output build/release-qualification/return-qualification/aggregate-certificate.json

scripts/agent release return-qualification verify-aggregate \
  --candidate PATH/TO/platform-v3.manifest \
  --layout public
```

The no-waiver minimum is three distinct boards, two distinct sink units using
at least two distinct sink chipsets, 300,000 transitions overall, and 100,000
transitions per board. The aggregate must
contain at least 150,000 arcade returns, 10,000 observations of every other
transition class, and 1,000 observations in every fixed release video mode.
Every underlying physical frame must remain correctly classified; one black,
stale, partial, banded, or corrupt frame rejects the entire candidate.

`scripts/agent release qualify` checks the default aggregate certificate against
the manifest of the exact active public or development launcher immediately
after attendance confirmation and before it arms any device mutation. Missing,
undersampled, malformed, tampered, or identity-mismatched evidence stops the
workflow without invoking restore because nothing has yet been changed.

## Hardware work still required

This interface validates and aggregates evidence; it does not claim that a lab
run occurred. Release qualification still requires the external frame-complete
HDMI/CRT capture and classifier, the board and sink sample matrix, and the
transition soak to produce the input documents. A trusted digital-signature
workflow is also required if evidence must be protected against a malicious
author rather than accidental modification. Timing, CDC, resource, and post-fit
RBF equivalence remain separate FPGA signoff gates.
