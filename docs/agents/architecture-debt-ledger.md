# Remaining architecture and qualification work

This backlog was reconciled against implementation baseline
`367823f48fe55e75b952384d1b43357fe38a8533`. Completed migration narratives are
available in Git history; they are not current command or assurance policy.
Root `AGENTS.md` owns validation boundaries.

## Qualification still requires evidence

The previous P0 handoff did not establish final qualification. A fast-gate pass
is insufficient: retain the full affected CI requirement until evidence is
attached to the exact candidate revision. Two benchmark routes remain recorded
qualification debt from the baseline and P1 Enforce comparison:

- `launch-return` timed out after 20 seconds waiting for the initial launcher
  return-state capsule.
- `launcher-response` timed out at feedback completion; the Enforce run confirmed
  all 17 inputs at the latch but recorded only 10 feedback-hide completions.

These are historical unresolved observations, not a new reproduction against
current main. Closing them requires evidence for the exact current candidate.
Physical visibility and cadence claims still require attended device evidence.

## Test coverage to reassess

The retired frontend audit left follow-up questions about loop-level launch
failure recovery, startup/display ordering, controller-setup disconnection,
production-shaped catalog fixtures, preview pack/index coherence and corrupt
payloads, and rendering/frame-budget coverage. Reassess these against current
code and tests before scheduling work; the old audit does not prove a current
defect or a current coverage gap.

## Runtime configuration

`apps/mister/config/runtime-environment.toml` remains the sole registry, with
`docs/reference/mister-runtime-environment.md` generated from it. Process startup
already captures immutable readiness and fault configuration. Remaining direct
registered reads still need typed subconfigs and process-boundary parsing.

Remove legacy read tolerance only after every process has one named parse site,
registered aliases/external inputs remain explicit, and negative tests reject
downstream reads. Keep the registry and generated reference; do not introduce a
second vocabulary. This is separate from behavior-preserving module extraction.

## Decomposition disposition

The former all-six-boundary extraction sequence is superseded. The discarded
host characterization and extraction work was not integrated; its parent branch
and worktree were removed. It is not a prerequisite for future tooling work.

| Owner | Revised disposition |
| --- | --- |
| Host workflows and device agent | Superseded by the replacement roadmap; stop the old decomposition sequence. |
| Launcher state and runtime | Defer until a concrete retained-application maintenance need justifies it. |
| Desktop app | Defer until the desktop's role alongside the new viewer is decided. |
| Catalog persistence | Defer until a concrete retained-application maintenance need justifies it. |

The existing architecture report remains a whole-family measurement tool for
future work; a shorter facade alone does not prove a reduction in complexity.
Existing shared typed UI globals, presenters, platform capabilities, and the
library-owned application module graph remain boundaries to preserve. Keep
ready-v2 posted-frame semantics and feature-local diagnostic allowances.

If a future bounded extraction is approved, preserve its observable sequence
tests and update affected source-path inventories in the same change. Do not
claim that historical static-check descriptions prove current enforcement;
verify the selected check implementation. This disposition does not waive any
qualification debt, hardware-interface safeguard, or active delivery contract.
