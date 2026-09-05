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
| LSP caps, compact responses, and branch/worktree routing | Keep and finish the identified error-path budget fix. |
| Completed host characterization and extraction commits | Preserve on the original cleanup branch; exclude from the focused integration branch. |
| Remaining host orchestration and old device-agent decomposition | Superseded by the replacement roadmap. Stop this work. |
| Launcher/runtime and catalog decomposition | Defer as independent application maintenance, justified by concrete future needs. |
| Desktop decomposition | Defer until the desktop's retained role alongside the new viewer is decided. |
| Hardware contracts and unresolved qualification evidence | Preserve. Replacement tooling does not make these obsolete. |

Six thin facades and completion of the original extraction sequence are explicitly
removed from this cleanup's acceptance criteria. Existing source complexity is
not reported as eliminated when the work is deferred.

## Ordered remaining work

1. **Preserve the existing branches and user changes.** Keep
   `nigel/agent-effectiveness` with its completed source extractions. Leave the
   original checkout and its unrelated private work untouched. Do not merge or
   rewrite that branch merely to tidy the history.
2. **Finish the bounded LSP correction.** Request-format and path-routing failures
   must obey the serialized structured-output budget, including oversized input.
   Preserve explicit failure information, full response compatibility, and
   complete rename/mutation responses. Test actual dispatch, not only a payload
   helper. Commit and push the independent LSP repository before updating a
   parent gitlink.
3. **Prepare a focused integration branch.** Create
   `nigel/agent-effectiveness-tooling` in a separate worktree from implementation
   baseline `367823f48fe55e75b952384d1b43357fe38a8533`. Select only the useful
   documentation and agent-tooling commits, plus this revised plan and the LSP
   correction. Exclude host characterization and extraction commits. Keep parent
   commits local for review.
4. **Reconcile documentation and status.** Mark superseded/deferred work clearly
   in the existing backlog and measurement documents. Do not create another
   startup instruction hierarchy. Do not apply future `magik2` development policy
   globally: active 1.0 workflows keep their applicable protections; future scoped
   instructions must distinguish the new development loop from legacy delivery.
5. **Verify and report the reduced scope.** Run selected Python tooling tests and
   focused Rust compatibility checks through `scripts/cargo`. Report instruction
   words/bytes, bounded LSP output, and available aggregate session evidence with
   their limits. Record how to start a fresh MCP session using the branch-local
   configuration. Do not claim hardware acceptance or productivity improvement.

The intended existing parent commit selection, before any new fixes, is:
`5339b1a6f`, `b1bc7737d`, `4776fb7cc`, `78514d9b7`, `91b43f645`,
`dc266d00f`, and `96405183f`. Cherry-picked commit identities will differ.

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
  unavailable evidence. The legacy refactor branch remains recoverable.

Terra with high reasoning performs the remaining implementation and tidy-up.
Each new implementation commit gets at most one review round; an identified fix
gets focused validation without a repeated review cycle. No parent push, merge,
deployment, destructive test, FPGA synthesis, dependency upgrade, or global
plugin change is included.
