# Raw-scaler boundary black-screen result — 2026-08-23

## Candidate

The disposable `raw-scaler-boundary-v1` observer was synthesized from the
patched FPGA inputs at commit `dee39545b`. The canonical local signoff report
was evaluated from commit `956364a75` and identified the profile as
`experimental_raw_scaler`.

- RBF SHA-256: `9f7fdd78041bf11638618f51e243157ed33db259081b283f1e90b21738c1192f`
- RBF metadata SHA-256: `b921da728008b1663a0617d4c4752dafe3147cc0e9c6d5b5bd1cc06c26613ebe`
- signoff report SHA-256: `28833829619d20d427b16609e38cc4a376a395af1da1e690f9a473e3b718a56b`
- device-agent SHA-256: `3eb6b21e4e284c342b942d30420c80530187b64516bad95224ce1a9036c900ac`

Patched-production GHDL, the exact completion proof, latch/sys-top Icarus, raw
observer simulation, and 41 Quartus-checker fixtures passed. Quartus 17.0
Build 595 seed 2 reported setup `0.389 ns`, hold `0.216 ns`, zero TNS, no new
warning class, `+109` ALMs, `+55` registers, unchanged RAM/DSP/PLL identity,
19 exact diagnostic CDC paths, and all three custom chains above `10^12`
device-hours. This is an attended diagnostic result, not production signoff.

The matching schema-2 agent was installed first against the restored schema-1
RBF, then the RBF was installed through the rollback-capable experimental
transaction. The final activation passed. Destructive arming state remained
clear.

## Baseline

The first post-install evidence was coherent `raw_scaler_active`: frame deltas
were `2,1`; every active and nonzero counter saturated at 15; latch ownership
was stable; and there were no latch drops or rejects. Native USB video showed
the complete MagiK home screen.

- baseline diagnostic SHA-256: `2da673071442442345b5f711474b55b2b0d708d2f0482bfd1db6a2b24f173948`

## Captured occurrence

The normal benchmark surface refused to run because a local experimental RBF
does not match the qualified delivery tuple. No delivery was run because that
would replace the observer. The typed attended Arcade launcher automation was
used instead. It timed out after 300 seconds without leaving the home screen;
its final semantic state reported action sequence zero and a transient cached
catalog count of zero.

The first native USB still after that timeout was physically black except for
the top-right clock. No restart, RBF reload, route mutation, or recovery action
was issued. Two independent diagnostics bundles were then captured, followed
by an authoritative latched-framebuffer capture.

- black/clock-only USB still SHA-256: `813646ce52bc134df6b7e8a748d97e0642515eef39acf4a0d26fd300f4312184`
- authoritative framebuffer SHA-256: `7d99d905cff17ee7508ebf7b4dd6135268f81b0bd822b2b39587bec906290fe4`
- diagnostics A SHA-256: `d9fcd4b4a517616194aca874111af11e24d2cea6db964249c3b2bddead144e53`
- diagnostics A bundle SHA-256: `acde2ae7b0a1685b0c91f71498efaf1994ddf684d29f71e7ccbba7ca6e7d02c9`
- diagnostics B SHA-256: `276555d40ecf37b5ed8c1cba86b007e0c8d15bf2371c6900a9382f3f490898b4`
- diagnostics B bundle SHA-256: `09b610fc362e0d1c6ecbd92fafa8939c76d6f034b4e5708e0d845fb97d64c9a0`

Both bundles classified `raw_scaler_active`. Their frame deltas were `1,2`,
CE and HS were seen in every sample, and all active/nonzero counters saturated
at 15. Latch ownership remained stable; sequence, transaction, route epoch,
post count, and flip count advanced; drops and rejects remained zero. The
authoritative framebuffer contained the complete expected 922-game home
screen.

A later explicit USB still showed the complete home screen again without any
recovery action. Therefore the physical black/clock-only presentation was
transient. The repeated observer reads and framebuffer capture were close in
time but not synchronized to the exact black physical frame; they must not be
described as frame-locked proof.

## Attribution

This occurrence does not support a persistent raw-scaler timing stall, missing
DE, or persistent all-zero raw output. Around the occurrence the raw boundary,
latch, and framebuffer all continued to advance and contain nonzero data.

The present probe cannot prove that the raw scaler emitted a spatially complete
frame. Its four-bit count saturates after 15 nonzero active samples, and the
visible clock alone contains enough pixels to saturate it. Consequently
`raw_scaler_active` distinguishes all-zero/stopped output but cannot distinguish
the complete UI from a clock-only or narrow-region raw frame. That limitation,
not the transport, is the remaining diagnostic gap.

## Next decision

Retire Design 5 after this evidence is archived. Do not add recovery controls,
pixel buses, PLL state, or more Avalon/scheduler state to it. If another RBF is
authorized, replace this probe with one small HDMI-domain spatial-coverage
observer at the same raw-scaler boundary: bounded nonzero occupancy for several
widely separated active-frame regions plus the existing frame heartbeat. Its
single purpose is to distinguish complete raw spatial coverage from a
clock-only/banded/partial raw frame. Only if raw spatial coverage is complete
should the following experiment move downstream toward the final mux/output
boundary.

The diagnostic RBF remains experimental and is not eligible for CI or release.
