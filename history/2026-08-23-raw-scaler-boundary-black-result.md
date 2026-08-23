# Raw-scaler boundary phase-2 invalid attempt — 2026-08-23

> Correction: the clock-only USB still described below was incorrectly called
> a reproduction of the target black-screen failure. The physical operator
> rejected that classification. No core launched and no return transition
> occurred, so this attempt provides no root-cause attribution.

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

## Invalid phase-2 attempt

The normal benchmark surface refused to run because a local experimental RBF
does not match the qualified delivery tuple. No delivery was run because that
would replace the observer. The typed attended Arcade launcher automation was
used instead. It timed out after 300 seconds without leaving the home screen;
its final semantic state reported action sequence zero and a transient cached
catalog count of zero.

The first native USB still after that timeout contained only the top-right
clock. It was initially misclassified as the target black-screen failure. That
classification is invalid: the launcher automation never performed a core
launch or return, the still was not confirmed by the physical operator as the
reported failure, and a later capture showed the normal UI without recovery.
Two independent diagnostics bundles and a latched-framebuffer capture remain
useful only as records of this failed harness attempt.

- invalid clock-only USB still SHA-256: `813646ce52bc134df6b7e8a748d97e0642515eef39acf4a0d26fd300f4312184`
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

A later explicit USB still showed the complete home screen without any
recovery action. The repeated observer reads and framebuffer capture were not
synchronized to a valid target failure and must not be used as black-screen
evidence.

## Attribution status

No attribution can be made from this attempt. It established only that the new
observer, decoder, transactional installation, baseline physical output, and
read-only evidence collection work. Phase 2 did not execute a valid return
transition and did not reproduce the reported black-screen failure.

The known probe limitation still applies: its four-bit count saturates after 15
nonzero active samples, so `raw_scaler_active` does not prove spatial
completeness. That limitation should be considered only if it affects a future
operator-confirmed failure capture; it is not a result from this attempt.

## Next decision

Keep Design 5 installed as the experimental diagnostic candidate. Correct the
phase-2 launcher/catalog harness without changing the RBF, then run valid
bounded core launch/return transitions. Stop only on an operator-confirmed
physical black, banded, or corrupted screen and capture three `0x67` records,
the authoritative framebuffer, and native USB evidence before recovery.

Do not design another FPGA observer from this invalid attempt. The diagnostic
RBF remains experimental and is not eligible for CI or release.

## Subsequent operator-confirmed moving-band incident

After the launcher harness was corrected, 55 uninterrupted valid returns
passed without reproducing black. An attended reboot then completed despite
the host command being interrupted, and the fresh MagiK Home presentation
entered the rare continuously moving spatial-corruption state.

The physical operator confirmed the failure. A 30-second native movie, every
one of its 732 captured frames, a temporal row-change heatmap, the correct
authoritative framebuffer, and two coherent `raw_scaler_active` snapshots were
preserved before recovery. Unlike the invalid attempt above, this is a valid
physical incident tied to fresh `LauncherActive` MagiK ownership. It proves
that Design 5's activity classification is too coarse to detect line/frame
phase integrity, but it does not yet locate the fault or establish that the
black-screen mechanism is identical.

The exact hashes and unrecovered-state record are in
[`2026-08-23-moving-band-corruption-result.md`](2026-08-23-moving-band-corruption-result.md)
and its schema-v1 integrity manifest.
