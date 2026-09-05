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

## Decomposition

| Owner | Remaining boundary |
|---|---|
| Host workflows | Device access/recovery, delivery, display/qualification, profiling, input/navigation, catalog/launch-return workflows. |
| Launcher state | Navigation/search/filter state and catalog presentation separated from persistence and platform effects. |
| Launcher runtime | Explicit frame phases, typed session state, diagnostic drivers, and ordered presentation/accounting. |
| Device agent | Portable request validation, authenticated transports, Linux services, and thin bootstrap. |
| Desktop app | Analytics/stream presentation, live/compiled bindings, and diagnostic modes. |
| Catalog persistence | SQL readers/projections separated from writers and transactional publication. |

Use the existing architecture report to measure whole owner families; a shorter
facade alone does not prove completion. Existing shared typed UI globals,
presenters, platform capabilities, and the library-owned application module
graph are established boundaries, not migration targets. Preserve ready-v2
posted-frame semantics and feature-local diagnostic allowances.

Serialize changes to shared launcher composition, presenter, runtime-config,
installed-layout, and assurance interfaces. Every extraction must preserve its
observable sequence tests and update any affected source-path inventories in
the same commit. Do not claim that historical static-check descriptions prove
current enforcement; verify the selected check implementation.
