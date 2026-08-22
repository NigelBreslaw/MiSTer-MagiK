# Retained bridge launcher-response attribution

Date: 2026-08-22

## Scope

This is the focused failure attribution required before retaining roadmap Item
14. The installed revisions were MiSTer MagiK `9279ea87a773ddc353cc2bb0f0725ee581a68d4f`
and Main_MiSTer `639d3694e1b93660020e9587cd0fe27f0170ce4c`.
Production remained on replacement bridge models.

## Checklist

- [x] Added and delivered a consumed `launcher-response-retained` arm.
- [x] Preserved every production input, pulse, integrity, cadence, and catalog-adoption gate.
- [x] Ran the retained arm three times with a same-head replacement control.
- [x] Ran the proxy-v3 production input-integrity control after the first failures.
- [x] Identified one benchmark-arming defect independently of the retained model.
- [x] Identified the dominant armed-run failure phase.
- [x] Kept the retained selector disabled for ordinary production launches.

## Results

The first and third retained attempts never consumed their one-shot launch
environment. Their final status remained on the ordinary Arcade-selected Home
state with `start_screen=arcade`, `selected_item_id=menu:arcade`, and no response
run. No input had been injected, so these are benchmark-authority failures, not
candidate latency or correctness results.

The second retained attempt armed successfully but stopped at 2/9 confirmed
actions and 0/8 hidden receipts. Its one visible Apple II feedback event remained
outstanding. The same-head replacement arm produced the identical 2/9 confirmed,
0/8 hidden, one-outstanding result. The retained model therefore did not cause
the failure. Both armed traces reached the first eligible vblank for the first
Right action; the failed phase is after the first visible receipt and before the
remaining scheduled Right actions are accepted.

The production input-integrity control passed both its idle and 500 ms UI-stall
scenarios with 109/109 initial presses, 109/109 releases, a neutral final held
state, and zero loss, duplication, sequence gaps, journal overflow, proxy write
failure, or latch drops. Its artifact is
`build/agent-benchmarks/input-integrity/1787377569/summary.json`.

The retained bridge performance authority remains:

- baseline: `build/agent-benchmarks/bridge-model-churn/1787376724/summary.json`
- candidate: `build/agent-benchmarks/bridge-model-churn-retained/1787376749/summary.json`
- media replacements: 60 to 1 (-98.333%)
- media row allocations: 177 to 30 (-83.051%)
- media bridge plus raster: 749,161 us to 88,222 us (-88.224%)
- aggregate bridge plus raster: 1,763,494 us to 1,089,451 us (-38.222%)

## Recovery decision

The first bounded recovery will make arming authoritative before any input:
the host must observe the exact response run ID plus the intended Home/Computers
state. It may reconcile one fully unarmed launch with one restart, but must not
replay a launch after an exact run ID is observed. Driver evidence will then be
retained so an armed failure can distinguish emitted pulses from accepted proxy
actions. Retention remains blocked until both replacement and retained response
arms complete with exact visible-to-hidden pairing and zero outstanding feedback.
