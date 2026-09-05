# Tooling retirement

## Milestone 7: catalog experiment tooling removed

Based on `f4e09c025`, after merged PR #94. This removes the remaining catalog
experiment command family without porting a feature into 2.0 or touching a device.

### Removed interfaces and support

The following `device catalog` commands are gone: `fast-five-prototype`,
`fast-five-c64-experiments`, `fast-five-experiments`, `fast-five-pprof`,
`fast-refresh-pprof`, `fast-refresh-benchmark`, `fast-source-ab`, `fast-media-ab`,
and `fast-five-old-cold`. They now fail as unknown subcommands, not as missing
arguments. Their argument types, dispatch, mutation classification, cold-run
matrices, reboot/staging wrappers, completion polling, telemetry and exclusive
report/test helpers are deleted. No aliases or archived copies remain.

The standalone `five-system-catalog-prototype` executable and Cargo target are
also deleted, along with its exclusive snapshot-import/search-probe wrappers,
argument parsers, C64 artifact experiments and source/media comparison wrappers.
The catalog crate's optional `pprof` dependency and `profile` feature are gone.
Package-targeted dependency sync removes 38 lockfile packages, adds none, and
preserves all remaining package versions.

### Preserved behavior and owners

`fast_five_catalog` remains production code: shared snapshot formats,
publication, refresh, transport, fingerprints and runtime behavior are retained.
The other catalog executables remain. Ordinary inspection, queries, metadata
and screenshot qualification, ROM/Neo Geo audits, core listing and explicit
purge keep their existing CLI contracts.

Shared shard writer signatures and production durability/SQLite/search defaults
are unchanged. Only five variants constructed exclusively by the deleted
experiment and their unreachable branches are removed. A private scanner
comparison field is deleted; emitted games and launch plans are unchanged.
The existing nested-ROM regression now calls production discovery directly.
The C64 scanner fixture still needed by a retained subtree-pruning test is
preserved as test-only support.

Desktop, installed-agent startup, platform/release delivery, physical input
qualification, catalog publication and standalone trace analyzers remain under
their existing owners. The 2.0 core, protocol, scope guard, two-repetition checks
and 15-second journey allowance are unchanged.

### Validation and review

- Five device parser tests pass, including all nine rejected commands and the
  retained catalog command set; 117 retained host tests pass.
- Catalog: three snapshot/transport tests, 12 publication tests, five production
  scanner tests and 14 shard tests pass. Both affected crates pass focused
  Clippy checks with warnings denied; the consuming app library check passes.
- The new worktree needed its unchanged asset submodule initialized for the app
  check. No asset, gitlink or private content is included in this change.
- Compiler diagnostics were checked against source callers, including retained
  tests. Source-text tests and references to deleted entrypoints were searched
  explicitly. Histogram diff review confirms retained fast-catalog function
  bodies are unchanged; shared writer edits remove only unreachable variants.
- No deployment, Linux reboot, benchmark or profile ran. Full cross-platform
  assurance remains CI's responsibility.

The host module falls from 18,483 to 15,416 lines. The milestone removes roughly
5,900 lines net before this documentation; future retirement should continue
from actual remaining consumers, not by migrating the deleted experiments.

## Milestone 6: legacy application test/benchmark retirement

Based on merged PR #93 (`929ae3e3c`). Implementation is on
`nigel/retire-legacy-test-bench`. Review corrections are complete. The user
approved a 15-second whole-sequence allowance; the final profile acceptance
passed at 13.81 seconds. The ten-second device profile remains a sample within
the journey, not full-sequence coverage.

### Removed

- The legacy application benchmark registry, route/arm orchestration,
  scheduler/storage attribution and their exclusive host helpers/tests.
  `host/mod.rs` falls from 44,699 to 18,521 lines. This is deletion, not a port.
- The old Python application UI harness and input bridge, device UI-test
  session implementation, protocol types/capabilities and dedicated legacy
  UI-test/catalog-prototype build intents. The real app's `ui-device-tests`
  feature and build profile remain because 2.0 uses them.
- `agent alpha accept`, its only release caller, and exclusive distribution
  archive/launcher helpers. This deliberately retires that acceptance ladder;
  the new scenarios do not claim equivalent published-alpha, HDMI, or physical
  input coverage. Separate release qualification remains.
- The duplicate frame-profile report/chart/heatmap/histogram/index/compare
  generators and schema tests. The three standalone raw-trace analyzers under
  `scripts/bench/analyze` remain pending a separate consumer decision.
- Root Python dependencies used only by the old harness. The independent 2.0
  Python environment keeps its own Slint dependency.

The approved compiler-backed helper deletion was followed by a production
Cargo check and compilation of all retained host tests. Retained code in the
large host module is unchanged apart from imports and deletion of the unused
`IniEdit::RestoreMain` variant/arm. Use Git's histogram diff to review the large
removal; its default matching algorithm misleadingly pairs repeated blocks.

### Replacement and remaining owners

Two small real-app journeys use the existing Python scenario framework:
Arcade selection/Home and Reduce motion toggle/readback/restoration. Each runs
twice when explicitly selected; default smoke remains unchanged. Their result
records include host-observed response times, including RPC/polling/screenshot
work, rather than device frame latency. No additional benchmark runner, core
API, manifest version, or service capability was introduced.

Arcade's Rust-painted list exposes its one-based selection through accessibility.
Discrete UI navigation now moves one row; physical input retains held scrolling.
A portable regression checks press/release behavior and list boundaries.
Slint screenshots capture the shell, not the Rust-painted game rows, and are
not physical framebuffer evidence.

Retained legacy owners are physical input integrity, Main/core-return,
FPGA/platform/release, Desktop Analytics, distribution/CI, catalog/media
publication, and installed-agent startup. Only `benchmark input-integrity`
remains in the legacy benchmark CLI. The installed old agent and startup files
have not been uninstalled or replaced by this milestone.

### Evidence and review

- Focused host CLI: 18 tests passed; delivery: 23 passed; input-integrity
  evaluator: one passed. All retained host tests compile with warnings denied.
- Portable Arcade input regression: one passed. Old device-agent host test
  compilation passed. CI follow-up also reproduced and corrected a Linux-only
  unused import through the typed `device-agent-ci` ARM build, which now passes.
- Scenario helper tests: 17 passed. Affected Python CI tests: 36 passed and two
  subtests. Focused Python lint/format and whitespace checks passed.
- Dev hardware: smoke passed; settings twice passed with original value
  restored; catalog twice passed after fixing the UI input path. Setup failures
  and several catalog diagnostic attempts preceded these passes; this was not
  a two-attempt development session. No Linux reboot was used.
- Final catalog evidence: `20260905T194251Z-c90f1d8ddf65`. Settings evidence:
  `20260905T190409Z-72463e50928e`, under ignored `build/magik2-results`.
- The initial profile attempt (`20260905T194452Z-54d2c12ba3d9`) completed both
  journeys but exceeded the original 12-second allowance at 13.54 seconds.
  Work paused for the user; no performance tuning loop followed. The user
  approved 15 seconds and deferred optimisation. This was a test allowance
  issue, not evidence of a runtime performance regression.
- Final profile acceptance (`20260905T195426Z-308dd0635f1e`): passed at
  13.812 seconds, including warm-up, remote calls and restoration. The
  ten-second sample, folded stacks and flamegraph were retained. Both journeys,
  original setting/selection restoration, and session cleanup passed.
- Review corrections: catalog chooses direction from its observable position
  and restores selection, including on capture failure; menu focus accepts a
  target reached on the last allowed step. Focused boundary/failure tests pass.
  The accessibility update copies only the selected index, without allocating
  game-path strings on the direct-rendering loop.

CI follow-up reproduced three missed failures locally: an orphan source-text
attribution test, Linux-only unused process imports, and consumer tests/docs
placed in protected core paths. The orphan test/imports are deleted. New
journey regressions live in `scripts/tests/test_magik2_journeys.py`; their
instructions live in `docs/benchmarking.md`. The tooling scope guard and label
policy are unchanged. Validation passed: 124 affected host tests, agent Clippy,
six relocated journey tests, and the typed ARM device-agent build.

The completed change removes about 39,200 lines net. No new 2.0 core API,
protocol/manifest version, or compatibility wrapper was added. Profile results
explicitly label their sample scope and record whole-sequence elapsed time.
Earlier inventory sections below describe the milestones at their completion;
they are superseded by this section for current retirement status.

## Milestone 5: one everyday application workflow

Based on merged PR #92 (`71ee7eb71`). Ordinary deployment, smoke, observation,
measurement and profiles use the existing `scripts/magik2` commands for both
real MagiK and Mini-MagiK. No 2.0 implementation or scenario changed.

Removed `deliver runtime`, `restart-ui`, bare `deliver`, and bare `benchmark`.
The app-only delivery switch, scope check, restart helpers and exclusive tests
are deleted. Removed commands fail during parsing; no aliases forward them.
The existing broader delivery transaction is now explicitly `deliver platform`.
It still reconciles platform/runtime/no-op decisions, including database input;
`deliver local-main` and `deliver game-databases` remain separate targets.

Active build/test/profile instructions point to 2.0. Retained platform/recovery
instructions and diagnostic messages name the explicit platform target. Direct
caller searches found no automation requiring the removed app-only commands.
Historical milestone records remain historical, not operational instructions.

### Remaining consumers

- Platform delivery still needs shared runtime building, manifests, transfer,
  lifecycle and recovery. Removing its app-only entrypoint cannot delete those
  shared implementations. Production packaging and Main/FPGA are unchanged.
- Specialized input, catalog, renderer and hardware benchmarks remain explicit.
  Smoke/idle scenarios do not establish their claims; none were ported or run.
- Attended UI qualification and its input bridge remain release consumers.
  Their ladder is explicitly outside ordinary application development.
- Desktop Analytics, distribution/CI artifacts, catalog/media publication and
  device startup still depend on legacy components. The installed old agent
  cannot yet be uninstalled as a consequence of this milestone.

### Verification

Host-only CLI parser tests: 19 passed. Focused delivery tests: 23 passed,
including retained database/local-Main coverage. Guidance tests: eight passed
and six subtests. Affected Python lint/format and whitespace checks passed.
Rust LSP was unavailable; source review and focused Cargo supplied validation.
No device access, ARM build, reboot, benchmark or new performance claim.

The implementation removes 114 source lines net (including tests and guidance).
No manifest/protocol version changes, dependencies, compatibility shims or new
features were introduced. Full assurance remains CI's responsibility.

## Milestone 4: obsolete experiment orchestration removed

Completed from main after PR #91. This is the earlier deletion record following
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
