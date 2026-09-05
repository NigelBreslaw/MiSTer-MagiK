# Agent effectiveness cleanup — revised scope

The MiSTer MagiK Tooling 2.0 roadmap supersedes the original 31-step cleanup
series. The replacement has an independent Python host, a small native device
agent, a browser viewer, and a deliberately small first development loop.
Reorganizing the old orchestration before retiring it is no longer a completion
requirement.

This plan covers conservative cleanup only. It does not authorize implementation
of Tooling 2.0, device operations, migration, deletion of active tooling, or a
change to production delivery policy. The roadmap's first-version implementation
requires its own bounded task.

## Disposition of the original work

| Work | Revised disposition |
| --- | --- |
| Instruction reductions and validation-policy corrections | Keep, preserving consequential safeguards and active legacy contracts. |
| Bootstrap-free guidance and actual ancestor discovery | Keep the capability; its legacy command alias need not survive eventual retirement. |
| Focused validation recommendations and privacy-safe context metrics | Keep; adapt ownership when a new project actually exists. |
| LSP caps, compact responses, and branch/worktree routing | Keep, including the completed error-path budget fix. |
| Completed host characterization and extraction commits | Discard from integration; the old parent branch and worktree have been deleted. |
| Remaining host orchestration and old device-agent decomposition | Superseded by the replacement roadmap. Stop this work. |
| Launcher/runtime and catalog decomposition | Defer as independent application maintenance, justified by concrete future needs. |
| Desktop decomposition | Defer until the desktop's retained role alongside the new viewer is decided. |
| Hardware contracts and unresolved qualification evidence | Preserve. Replacement tooling does not make these obsolete. |

Six thin facades and completion of the original extraction sequence are explicitly
removed from this cleanup's acceptance criteria. Existing source complexity is
not reported as eliminated when the work is deferred.

## Focused tidy-up status

The bounded LSP correction, focused integration branch, documentation
reconciliation, and selected verification are complete. See
[`agent-effectiveness-status.md`](agent-effectiveness-status.md) for the branch
selection, discarded refactor work, LSP activation boundary, and deferred work.

Future work must be a separate bounded task for a retained system or a project
with its own scoped instructions. Do not infer a broader migration, delivery
policy change, or hardware qualification result from this completed tidy-up.

## Verification and completion

- Guidance retains ancestor/override discovery, authority mappings, and a
  bootstrap-free human/JSON interface.
- Planning remains read-only and reports unresolved coverage instead of guessing.
- LSP navigation and failures respect supported budgets; compact/full selection,
  one-based evidence, rename preconditions, and registered-worktree isolation
  remain covered. A suitable fresh MCP session can navigate branch/worktree files.
- The focused branch contains no host, old device-agent, launcher/runtime,
  desktop, or catalog implementation decomposition.
- Instruction targets remain editorial targets, not grounds to remove safeguards.
- Documentation separates retained changes, superseded work, deferred work, and
  unavailable evidence. The discarded refactors are not an integration dependency.

Terra with high reasoning completed the conservative implementation and tidy-up.
Each new implementation commit received at most one review round; identified
fixes received focused validation without repeated review cycles. The tooling-only
branch is submitted for PR review after rebasing onto current `main`. Merging,
deployment, destructive tests, FPGA synthesis, dependency upgrades, and global
plugin changes remain outside this cleanup.
