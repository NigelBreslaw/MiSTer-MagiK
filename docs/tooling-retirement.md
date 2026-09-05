# Milestone 4: obsolete experiment orchestration removed

Completed from main after PR #91. This is the current deletion record following
[the milestone 3 consumer inventory](../magik2/docs/legacy-disposition.md).
Nothing was ported into Mini-MagiK or the 2.0 host/service.

## Removed interfaces

- Top-level `live-particles`, `startup-particles` and `scene-lab`, including
  preview, capture, cabinet-codegen analysis and its 27-case experiment matrix.
- Their attended `device` equivalents, upload/session/restoration orchestration,
  recipe watching and experiment-specific profiling wrappers.
- Lab-specific build intents and four ARM/macOS compile-time targets, plus edit
  selectors that no retained target supports.
- The five particle benchmark aliases that only redirected to the retired lab:
  particles, particle-capacity, particle-demo-40k, particle-step and particle-profile.
- The 12-session UI profile matrix, its suite entries, protocol case allow-list
  entry, release-suite default selection and instructions.

Removed CLI surfaces produce normal unknown-command/value errors. There are no
compatibility stubs, aliases or archived copies of removed source.

## Preserved boundaries and next deletion decisions

| Consumer / capability | Current disposition |
|---|---|
| Production particle effects, renderer libraries and generated intro assets | Preserved unchanged. |
| Standalone lab application source | Preserved; its legacy host orchestration is deleted. No new preview/deploy wrapper was added. |
| Intro asset generation | Standalone Cargo binary; see [regeneration instructions](startup-particles.md#intro-asset-regeneration). |
| Everyday 2.0 deploy, smoke, viewer and profiles | Unchanged. |
| Desktop Analytics and its old-agent client | Still a legacy consumer; requires a separate feature decision before migration or deletion. |
| Production packaging, Main/FPGA delivery and catalog/media publication | Retained under their existing owners. |
| Full-app compile measurements, scheduler/storage attribution and offline frame-profile reports | Retained; this milestone does not establish that their consumers are obsolete. |
| Remaining focused legacy UI journeys | Retained; only the cross-product matrix is removed. |
| Installed old agent and startup configuration | Untouched by this milestone. No device operations ran. |

The generator's existing lockfile was stale against the current catalog
manifest. The owning-package dependency-sync command added only `hotpath` and
`hotpath-macros` entries and their existing dependency references. No manifest,
generator source, font or checked-in asset changed.

## Verification and review

- Focused host Cargo check passed; CLI parser suite passed 19 tests. Retained
  delivery, release and platform parser coverage stayed in the suite.
- After removing the last unused edit selectors, the focused retirement parser
  regression passed again. It rejects all removed command/build/benchmark/edit
  surfaces before execution.
- Affected Python suite-selection and guidance tests: 12 passed, six subtests.
- Standalone intro generation succeeded once into a temporary directory; all
  four particle files and `PROVENANCE.txt` matched checked-in files byte for byte.
- A source search found no dangling experiment module/type references. Current
  instructions no longer invoke retired agent commands. Renderer documents keep
  historical technical evidence with an explicit retirement note.
- Python formatting/lint and whitespace checks passed. Rust LSP was unavailable
  in the session; source inspection and focused Cargo supplied validation.

Initial parser validation caught stale expectations and an orphaned enum naming
attribute; both were corrected before the passing run. The generator initially
stopped at its stale locked dependency graph, before generating any files;
verification succeeded after the focused lockfile repair.

No ARM builds, MiSTer connections, application restarts, hardware acceptance,
performance measurements or broad local Cargo assurance were run.

## Size

Source, including embedded and standalone tests: **70 lines added, 4,120 deleted;
4,050 lines removed net**. The lockfile adds 16 lines. Documentation: **124 added,
128 deleted**. Overall: **210 added, 4,248 deleted; 4,038 removed net**. No moved
files or generated assets inflate the source-deletion count.
