# Milestone 3: the everyday development path

## Scope and commit plan

1. Make `scripts/magik2` default to real MagiK for build/deploy/check/watch;
   Mini remains explicit with `--app mini-magik`. Default checks run one smoke
   journey, not benchmark windows. Print the selected executable/data paths and
   retained failure evidence. Update root developer guidance.
2. Exercise Home → Settings → Home using Slint keyboard events and the existing
   typed application action queue. Retain response timings and a screenshot in
   the same pytest result, without a second benchmark runner or performance gate.
3. Correct Linux process-name truncation in the legacy-agent status check, then
   verify the ordinary workflow while the old agent is absent. No uninstall or
   persistent startup changes. Add only focused regression checks.
4. Record acceptance and a source-backed legacy consumer disposition inventory.

## Definition of done

The ordinary CLI targets the Dev app/data explicitly; smoke exercises real
navigation; a failure points to retained logs; bounded device acceptance records
legacy-agent absence before and after. Existing idle profiling and viewer paths
are exercised once only as needed to establish independence. No transfer or
Mini benchmarks, performance tuning loops, platform changes or broad local
assurance. Necessary app restarts are authorized.

## Explicit exclusions

Do not port matrices, endurance loops, workflow databases, qualification ladders,
rollback gates, version matching, historical experiments or separate benchmark
runners. FPGA production, downloader publication and data releases stay outside
this development tool. The consumer inventory is a deletion decision list, not
an implementation backlog. The legacy CLI and installed agent are not uninstalled
in this milestone.

## Acceptance

Completed on 5 September 2026 against `192.168.1.117`.

| Check | Result |
|---|---|
| Focused host validation | 56 tests passed before the final stop/acceptance additions; affected tests then passed 17/17 in 0.22 s. |
| Native status regression | One focused Cargo test passed, including compilation of the final stop implementation. |
| ARM build | Real app build passed (3m11s in the fresh worktree); a subsequent changed-input build during smoke took 73.445 s. Native updates took 6.48 s and 2.69 s. |
| Correct legacy status | The fixed check reported the old agent running; the earlier negative was incorrect. |
| Explicit legacy stop | Passed once; observed absence confirmed after acknowledgement. No startup files changed. |
| Default command | `scripts/magik2 check`: one smoke passed, three measurement/profile cases deselected; 99.26 s including build, upload, launch and cleanup. |
| Real interaction | Home → Settings → Home passed. Settings screenshot inspected; no setting was changed. All reported paths were Dev. |
| One optional profile | `check idle --profile`: one case passed, three deselected in 24.32 s. Build reused in 24 ms; ten sampled stacks plus folded output/flamegraph retained. |
| Brief native observation | One decoded 960×540 frame and matching metrics; old agent absent before/after, same app PID 18235. No restart for this check. |
| Local hygiene | Python lint/format, tooling scope check and whitespace checks passed. No broad local Cargo assurance. |

The navigation response observations were 330.65 ms to open and 936.62 ms to
return. They include host RPC, accessibility reads and polling. They are neither
device frame latency nor a comparative performance baseline, and were not used
for tuning. The ten idle profile samples establish the pipeline, not hotspots.
No transfer, motion matrix or Mini benchmark was run. Normal session cleanup
passed and restored the persistent real development app.

Evidence under ignored `build/magik2-results/`:

- Legacy stop: `20260905T170744Z-6c19226c4753`
- Default smoke/navigation: `20260905T170849Z-3fc368a1af8e`
- Profile: `20260905T171058Z-f79a7df9dbd3`
- Independent stream/final status: `20260905T171304Z-e86527b74417`

A first standalone stream-check invocation lacked the host module path and
failed before contacting hardware; the corrected invocation passed. The attached
MCP process still predates the merged LSP startup fix and rejected this worktree;
that is an existing-session issue, not a missing rebase. Source inspection,
focused Cargo validation and the actual ARM build supplied validation here.

## Review and retained boundaries

The default command does not run measurement repetitions. Mini's retained
acceptance harness now selects `--app mini-magik` explicitly, preventing the
changed default from accidentally deploying the real app during those checks.
Build/deploy/watch share the existing application registry; no new project
manifest or matching-version gate was added.

Development keyboard events feed the existing bounded UI action queue. They
are enabled only by the real app's `magik2` feature; production builds reject
them. The smoke asserts the Settings main landmark, rather than mistaking the
Settings button for the open screen, and attempts Back after screenshot failure.
This exercises application navigation, not a physical controller button.

The new legacy stop is explicit and never called by deploy/check/watch. It
validates the executable and process birth identity, sends TERM once, waits at
most three seconds after signalling, and reports failure if still running or
respawned. It neither escalates to KILL nor edits startup configuration. A future
reboot can start the old agent again. The corrected status capability triggers
an automatic service upgrade only when that operation needs it.

The [legacy disposition inventory](legacy-disposition.md) identifies what to
delete and what still has active consumers. Desktop Analytics and production/CI
agent packaging are real remaining dependencies; they were not ported. The
old agent is stopped on this device, not uninstalled across the project.

All planned implementation and bounded acceptance are complete. No further
hardware runs are needed for this milestone.
