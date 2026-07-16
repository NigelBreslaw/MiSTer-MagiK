# Commit 06: truthful cold-preview readiness phases

## Scope and confirmed cause

Correctness-only benchmark instrumentation change; no performance claim.

The cold-preview parser captured the candidate event's
`selected_has_preview` field as though it described the discovered candidate.
It also treated a system with no candidate/request/decode/apply path as
`pass=1`, silently combining an absent asset with a successful latency result.

Immediate parent: `e47e5eddab5906e3c355f034a191c2e583062ddb`.

## BEFORE contract

```text
scripts/gate-cold-preview-systems.sh C06-COLD-PREVIEW-E47E5ED-BEFORE --systems arcade,neogeo,saturn
```

The command exited 1 because Arcade and Neo Geo missed the existing 32 ms
request-to-apply budget. Arcade reported 114 ms and Neo Geo 40 ms. Saturn had
no asset key and no request/decode/apply phases, but was incorrectly emitted as
`pass=1` rather than an explicit skip.

Raw artifacts:

- `build/preview-state-profiles/C06-COLD-PREVIEW-E47E5ED-BEFORE-arcade.log`
- `build/preview-state-profiles/C06-COLD-PREVIEW-E47E5ED-BEFORE-neogeo.log`
- `build/preview-state-profiles/C06-COLD-PREVIEW-E47E5ED-BEFORE-saturn.log`

## AFTER contract

```text
scripts/gate-cold-preview-systems.sh C06-COLD-PREVIEW-E47E5ED-AFTER --systems arcade,neogeo,saturn
```

The command still exits 1 and the gate is not weakened. It now reports target
list readiness, candidate discovery, selected request, decode, and apply as
separate fields:

- Arcade: real candidate, request/decode/apply present, 152 ms,
  `result=fail`, `failure_reason=request_to_apply_budget`.
- Neo Geo: real candidate, request/decode/apply present, 74 ms,
  `result=fail`, `failure_reason=request_to_apply_budget`.
- Saturn: no candidate asset and no request/decode/apply,
  `result=skip`, `skip_reason=no_preview_candidate`.
- Aggregate: requested 3, passed 0, skipped 1, failed 2.

The nonzero result is the truthful outcome for the current device assets and
latencies. This item makes no performance claim.

Raw artifacts:

- `build/preview-state-profiles/C06-COLD-PREVIEW-E47E5ED-AFTER-arcade.log`
- `build/preview-state-profiles/C06-COLD-PREVIEW-E47E5ED-AFTER-neogeo.log`
- `build/preview-state-profiles/C06-COLD-PREVIEW-E47E5ED-AFTER-saturn.log`

## Validation

- `scripts/profile-cold-preview-systems.sh --self-test`
- `scripts/gate-cold-preview-systems.sh --self-test`
- `bash -n scripts/profile-cold-preview-systems.sh`
- `scripts/dev-rust test` (283 passed)
- `scripts/dev-rust check`
- `cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings`

The change is limited to production benchmark/correctness scripts and
benchmarking documentation. It does not touch the device binary, experimental
effects, or reboot/fault arming paths.
