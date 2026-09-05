# Remaining architecture and qualification work

This backlog was reconciled against implementation baseline
`367823f48fe55e75b952384d1b43357fe38a8533`. Completed migration narratives are
available in Git history; they are not current command or assurance policy.
Root `AGENTS.md` owns validation boundaries.

## Qualification still requires evidence

The previous P0 handoff did not establish final qualification. A fast-gate pass
is insufficient: retain the full affected CI requirement until evidence is
attached to the exact candidate revision. The two incomplete benchmark routes
recorded in `history/2026-08-15-p1-enforce-acceptance.md` remain qualification
debt; this cleanup neither resolves nor waives them. Physical visibility and
cadence claims still require attended device evidence.

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

The former all-six-boundary extraction sequence is not active work for this
cleanup. Preserve the completed host characterization and extraction commits on
`nigel/agent-effectiveness`, but do not treat them as integrated or complete on
the focused tooling branch.

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
