# Mailbox-aware zero-copy gate

## Confirmed cause

The max-scroll analyzer validated latch activity only through the legacy direct
latch `flip_count` and `post_count`. Atomic mailbox presentation intentionally
does not increment those counters, so a run with 84/84 valid mailbox-backed
frames was rejected even though backend, alternation, deadline, and FPGA error
checks all passed.

The analyzer now accepts either legacy post/flip progress or mailbox
`apply_count` progress, rejects nonzero mailbox errors, and handles mailbox
epoch changes caused by supervised module/session restart.

## Before

Reanalysis of `PROD-ZC-COHERENT-MAILBOX-20260711`:

- backend valid: 1
- latch frames: 84/84
- visual latch misses: 0
- reported gate valid: 0
- invalid reason: `latch_visual_or_backend`

## After

The same immutable trace and FPGA reports now produce:

- reported gate valid: 1
- invalid reason: `ok`
- mailbox apply count after epoch change: 758
- mailbox error count maximum: 0

Evidence:

- `build/launcher-home-scroll-profiles/PROD-ZC-COHERENT-MAILBOX-20260711-launcher-home-scroll.tsv`
- `build/launcher-home-scroll-profiles/PROD-ZC-COHERENT-MAILBOX-20260711-fpga-latch-before.log`
- `build/launcher-home-scroll-profiles/PROD-ZC-COHERENT-MAILBOX-20260711-fpga-latch-after.log`

Validation:

- `python3 -m py_compile scripts/analyze-max-scroll-drops.py`
- `scripts/analyze-max-scroll-drops.py --self-test`
- Immutable device trace reanalysis
