# Tooling retirement

## Milestone 9: visual retirement and remaining legacy audit

Based on `02387d891`, the merge of PR #104, on
`nigel/retire-visual-tooling`. This section is the current disposition; older
milestone sections below are historical records and can describe tools since
removed. No functionality is ported to 2.0 in this milestone.

### Deletion record

Removed `device launcher capture-first-arcade`, `capture-crt-font-ab`,
`capture-snes-hub`, `launch-return-once`, `verify-neogeo-sdram`, `device display
matrix`, and all `device scene` entrypoints (launcher, controller-test,
tear-pattern, video-playback and CRT-trial). Launcher restart no longer accepts
`--crt-font-experiment` or `--crt240-composition`. Unknown-command/argument
errors replace these interfaces; there are no aliases.

Deleted their exclusive staging, navigation, comparison, USB temporal luma,
core-return, report and test helpers, including the entire 1,473-line host
`launcher_automation.rs`. The standalone `launcher-present-trace.py`,
`analyze-max-scroll-drops.py` and `analyze-arcade-frame-trace.py` remove exactly
2,070 lines, including their embedded fixtures/self-tests. No external callers,
registrations, exclusive dependency packages or separate fixtures were found.
No Cargo manifest, lockfile or Python dependency changes were needed.

The two code-removal commits delete 6,166 lines and add 209 (5,957 net removed),
including one obsolete script-layout documentation line. Git's default diff
pairs repeated Rust blocks poorly; histogram review shows only the intended
restart dispatch simplification, imports and removal of an unused capture
timing field in retained production functions. Ordinary capture pixel handling
and USB-video handling are unchanged. Documentation changes are additional to
these implementation counts.
The complete reviewed diff, including this audit and parser refinements,
deletes 6,269 lines and adds 416 (5,853 net removed).

This deliberately drops automated font comparisons, fixed-screen fixtures,
the attended runtime multi-resolution matrix, SDRAM/core-return experiments and
these three trace reports. Current Python scenarios do **not** replace that
coverage. Add a focused scenario only when a concrete need justifies it.
Production rendering, trace producers/formats, frame measurements and CPU
profiling are unchanged. Single-mode display changes still use helpers named
`DisplayMatrix*`; those helpers have retained callers and must not be deleted
based on their names. The separately owned release display matrix also stays.

### Complete remaining CLI inventory

Source authority: `agent-cli/src/cli.rs`, `commands/device.rs`, `build.rs`,
`compile_time.rs`, `dependencies.rs`, `fpga.rs`, `model.rs`, and dispatch in
`main.rs`/`host/mod.rs`. All paths in this audit are repository-relative code
owners, not inferred individual maintainers. Hidden CLI leaves are included.
`deliver`, `benchmark` and `build` take positional enum values; every value is
listed below. Automatic Clap help/version is not an operational command.

Blockers: **C** legacy CLI; **A** device agent; **P** protocol crate; **S** startup
installation; **I** legacy agent CI/package artifact. A group marked **C** is
host-only and does not itself require A/P/S/I. **C + device** means C and the
shared legacy connection/health/bootstrap path (A/P/S/I), even when the final
operation also uses typed SSH or Main. Deleting a CLI leaf alone does not remove
its independently used library code. Tests are validation, not evidence of an
external consumer. “No caller found” means repository search, not proof about
private/operator usage.

| Remaining leaves (prefix `scripts/agent`) | Purpose and code owner | Consumer evidence and disposition | Blockers |
|---|---|---|---|
| `guidance PATH`; wrapper `plan` | Ownership/instructions and validation preview; `scripts/magik_ci/guidance.py`, `scripts/checks/pre-push.py`, `guidance.rs` | Root AGENTS and contributor workflow; retain separately. Wrapper routes these without compiling Rust. | Wrapper only; Rust duplicate guidance is a later candidate |
| `run show` (hidden) | Read a recorded run; `main.rs`, `evidence.rs` | Dispatch and tests; no current external invocation found. Later deletion candidate; do not delete shared run storage. | C |
| `db report` (hidden) | Report host evidence database; `main.rs`, `evidence.rs` | Explicit root AGENTS operator workflow; retain separately. | C |
| `diagnose` | Bounded recovery and diagnostics; `diagnose.rs`, `host/mod.rs` | `docs/device.md` boot-loop recovery; retain separately. | C + device |
| `device status`, `arming-status`, `logs`, `events`, `diagnostics` | Status, reboot arming and bounded evidence; `commands/device.rs`, `host/mod.rs` | `docs/device.md` operator workflows; retain separately. | C + device |
| `device transfer-check` | Explicit legacy upload/fetch throughput check; `host/transfer_check.rs` | Added for the old/new delivery comparison; parser/transfer tests remain, no ongoing scripted caller found. Later deletion candidate after the comparison is no longer useful. | C + device |
| `device mode status`, `mode set` | Dev/Public/Stock Main selection; `host/mod.rs` | Device operator instructions; retain separately. | C + device |
| `device display route-status`, `display set` | Inspect route; change one mode with attended restoration/confirmation; `host/mod.rs` | `docs/device.md`, display parser/readiness tests; retain separately. | C + device |
| `device crt qualify`, `crt probe`, `crt restore` | Physical CRT qualification/pattern probing and restoration; `host/crt_qualification.rs` | `docs/crt.md`, retained CRT implementation; retain separately as platform qualification. | C + device |
| `device capture framebuffer` | Legacy authoritative capture and derived 4:3 files; `host/mod.rs`, `host/framebuffer_views.rs` | `docs/device.md`; retain separately alongside 2.0 capture. Desktop uses the device endpoint directly. | C + device |
| `device reboot` | One attended supervised reboot; `host/mod.rs` | Device recovery instructions; retain separately. | C + device |
| `device launcher status`, `restart`, `return-to-launcher` | Ordinary Main-supervised launcher control; `host/mod.rs` | Device/Main operator instructions and platform recovery; retain separately. | C + device |
| `device catalog inspect`, `query`, `cores` | Inspect installed catalog, read-only query and core inventory; `host/mod.rs` | Catalog operator interface documented in `docs/device.md` and this inventory; parser/host tests verify it. Retain separately. | C + device |
| `device catalog metadata-qualification`, `rom-audit`, `neogeo-family-audit` | Validate published metadata, installed ROMs and NeoGeo family data; `host/mod.rs`, production `crates/catalog` | Explicit user retention decision and retained audit implementations/tests; retain separately. NeoGeo family audit does not launch cores or test SDRAM. | C + device |
| `device catalog screenshots`, `screenshot-qualification` | Export screenshot identities and validate packs against catalog; `host/mod.rs` | Published catalog/media qualification workflow and retained tests; retain separately. | C + device |
| `device catalog purge` | Explicit Dev data deletion with attended reboot; `host/mod.rs` | `docs/device.md` recovery; retain separately. | C + device |
| `device media check`, `media download` | Inspect/download configured media packs; `host/mod.rs` | `docs/media-download-security.md`, catalog/media operator use; retain separately. | C + device |
| `device fpga install-experimental`, `install-experimental-agent` | Attended FPGA candidate or matched legacy-agent activation; `host/mod.rs` | `docs/fpga-development.md`/platform workflows; retain separately. | C + device |
| `deliver platform`, `deliver local-main`, `deliver game-databases` | Qualified platform delivery, committed Main development, ordinary database publication; `delivery.rs`, `local_main_delivery.rs`, `database_delivery.rs`, `host/mod.rs` | Root AGENTS, `docs/main-mister-fork.md`, release/catalog workflows; retain separately. | C + device |
| `benchmark input-integrity` | Physical input proxy/kernel qualification; `benchmark.rs`, `host/mod.rs` | Explicit user decision and physical qualification contract; retain separately. Synthetic Slint input does not replace it. | C + device |
| `capture usb-video` | Native macOS AVFoundation still/movie capture; `capture.rs` | `docs/device.md`, attended hardware/release evidence; retain separately. No agent is needed for this host capture. | C |
| `release qualify` | Attended platform/latch/return gate, including release display matrix; `release.rs`, `host/mod.rs` | `docs/production-readiness.md`, `docs/fpga-latch-release.md`; retain separately. | C + device |
| `release frame-evidence verify` | Validate recorded frame evidence; `return_qualification.rs` | `docs/return-video-qualification.md`; retain separately, local file validation. | C |
| `release return-qualification record-board`, `aggregate`, `verify-aggregate` | Record/check board evidence and aggregate certificates; `return_qualification.rs` | `docs/return-video-qualification.md`, `release qualify` certificate consumer; retain separately. These leaves process evidence, not new device runs. | C |
| `compile-time build`, `measure`, `compare-revisions`, `campaign` | Full-app build timing and comparisons; `compile_time.rs`, `build.rs` | Implementation and tests remain; no current external scripted invocation found. User explicitly retains for now; further deletion deferred. | C |
| `clean` | Remove local Cargo artifacts; `clean.rs` | CLI help/implementation and tests; no external caller found. Later deletion candidate; shared build caches need a separate decision. | C |
| `dependencies sync` | Package-scoped lockfile maintenance; `dependencies.rs` | Root AGENTS, guidance workflow; retain separately. | C |
| `fpga setup`, `fpga signoff` | Apple-container Quartus installation and matched platform signoff; `fpga.rs` | `docs/fpga-development.md`, installer wrapper; retain separately. | C; signoff does not require a running agent |
| `build runtime-device`, `runtime-ci`, `runtime-analysis` (hidden) | Runtime build recipes; `build.rs` | Runtime baseline/CI specs also consumed by compile-time tools. No separate live CLI invocation established for each hidden value; retain pending build audit. | C; runtime artifacts are independent of old-agent removal |
| `build validate-launcher`, `validate-library`, `validate-runtime` (hidden) | Focused app checks and combined validation; `build.rs` | `execute_runtime_validation` uses launcher/library specs; tests cover recipes. No current external CLI caller found; defer with build audit. | C |
| `build device-agent`, `device-agent-ci` (hidden) | Build the retained service; `build.rs` | Legacy bootstrap needs the agent; current ARM CI uses Python `magik-ci build device-agent-ci`, not this hidden CLI alias. Keep while build audit is deferred. | C for aliases; A/P/S/I for the artifact |
| `build manager-device`, `release-binaries` (hidden) | Manager and packaged runtime builds; `build.rs` | `execute_release_binaries` consumes manager spec; production packaging remains. No current external invocation of these aliases found; defer alias decision. | C; keep manager/runtime packaging independently |

Compile-time detail: targets are `magik-full-app-arm`, `magik-full-app-macos`,
`magik-release-device-arm-all`, `magik-release-device-arm-production`,
`magik-release-device-arm-thin`, and `magik-release-device-arm-thin-stripped`.
Comparison scenarios are `pre-push-catalog` and `arm-runtime-ci`; edit selector
is `shared-magik`. Existing measurement code still requests one cold plus five
no-op plus five source rebuilds. That is audited legacy behavior, **not** a new
recommendation or permission to run those matrices. The 2.0 two-repetition
checks and 15-second journey allowance remain unchanged; no measurements ran.

### Device-agent dependency inventory

| Dependency group and code owner | Actual consumers / evidence | Disposition and removal blockers |
|---|---|---|
| Authenticated control/envelopes, failure classification, discovery and identity; `crates/agent-protocol`, `agent-cli/src/host/agent_client.rs`, `mister/tools/agent/src/main.rs` | Retained CLI connection/bootstrap and Desktop `apps/desktop/src/agent_client.rs` import the protocol. Ping/status are live requests. | Retain separately for platform CLI; Desktop migration deferred. Blocks A/P/S/I; CLI connection also blocks C. |
| Status, logs, timeline, diagnostics, crash/boot evidence and supervised reboot; device `main.rs`, host `mod.rs` | Retained diagnostics/recovery/release lanes and Desktop status polling. | Retain separately. Blocks A/P/S/I and retained C workflows. |
| Main `magik` control/status, acknowledged operations and handoff; device `main.rs` | Retained launcher/platform control; Desktop requests `magik` status. Some verbs may have no current UI caller, but shared control remains. | Retain separately; Desktop migration deferred. Blocks A/P/S/I. Do not remove production Main behavior. |
| SD directory/stat/MRA/image preview endpoints (`sd_list_dir`, `sd_list_dir_v2`, `sd_stat_item_v1`, `sd_parse_mra_v1`, `sd_read_preview_image_v1`); device `main.rs` | Desktop browser calls v2 listing with an explicit v1 fallback, stat, MRA parsing and binary previews. Both listing versions have real callers. | Desktop migration deferred. Blocks A/P/S/I; Desktop itself does not require C. |
| PNG/raw/LZ4 framebuffer captures and `framebuffer_stream_v1`; device `main.rs`, `host/framebuffer_views.rs` | Retained CLI capture; Desktop LZ4 seed and stream. Actual scanout/platform primitives and runtime producer are shared behavior. | Retain legacy path separately; Desktop migration deferred. Blocks A/P/S/I. 2.0 capture is already independent and does not justify deleting Desktop's protocol. |
| `device_telemetry_stream_v2`, analytics leases, frame/process counters and FPGA diagnostics; device `main.rs`, `crates/agent-protocol` | Desktop Analytics requests process telemetry; platform health and qualification consume diagnostic state. | Retain separately; Desktop migration deferred. Blocks A/P/S/I. Removing trace report scripts does not remove runtime trace formats or telemetry. |
| Native runtime upload; `mister/tools/agent/src/runtime_upload.rs`, host `agent_client.rs` | `host/transfer_check.rs` and retained `SshDeployRemote::put_runtime_binary` delivery path. | Retain separately with delivery; blocks A/P/S/I. |
| Device `launcher_automation_begin`/`launcher_automation_request`; `mister/tools/agent/src/launcher_automation.rs` | Repository request-name search finds only device dispatch after this milestone. Its capability is still required by host `agent_client::version_action`. Tests are not a consumer. | Later deletion candidate, **not deleted here**. Capability/connection cleanup is needed before removal; nominal compatibility requirement still ties it to retained C/A/P. |
| `alpha_candidate_install`; `mister/tools/agent/src/alpha_candidate.rs` | Request-name search finds device dispatch, no current repository client. Old alpha acceptance was removed earlier. | Later deletion candidate; keep this batch scoped. No demonstrated functional consumer justifies it as a permanent A blocker. |
| Service boot/install/repair; host `agent_client.rs` (`REMOTE_INIT`, bootstrap), device `main.rs` (`net-boot`) | Retained CLI installs `/etc/init.d/S00magik-agent`; Desktop assumes the service is reachable. Device boot/crash logging is part of the service. | Retain while these named consumers remain. Blocks S/A/I; cannot remove startup merely because 2.0 runs another service. |
| Agent build and distribution artifact; `.github/workflows/rust-arm.yml`, `scripts/magik_ci/build.py`, `scripts/magik_ci/distribution.py`, host bootstrap | ARM job `device-agent-arm-build` builds/uploads `mister-magik-agent-ci-fast`; legacy bootstrap/platform consumers still need a service binary. | Retain I while A/S are required. Python CI/release processing is separately owned, not something to port into 2.0. |
| Production catalog/media formats and publication; `crates/catalog`, `crates/media-contract`, host catalog/database modules | Real app, ordinary publication and audit consumers. `fast_five_catalog` remains production. | Retain separately even after eventual C/A retirement. Shared library formats are not legacy-service protocols to delete. |

### Later candidates and concrete completion blockers

The reachable CLI inventory above excludes the old block comment in `cli.rs`
containing `GameDatabaseCommand`, `PlatformManifestCommand` and platform bundle
argument shapes. These are commented-out declarations, not compiled APIs or
working CLI commands. That stale comment is a later documentation cleanup
candidate; underlying publication code is still used by separately owned Python
CI/release workflows.

The most direct later cleanup is device-side launcher automation and the
orphan alpha installer, plus their capability advertisements/requirements.
Hidden run inspection, transfer comparison and duplicate build/manifest command
surfaces also deserve individual consumer decisions. These are findings, not
additional deletions authorized by this batch.

Removing 1.0 completely is still blocked by three concrete groups: Desktop's
browser/Analytics/capture protocol; retained platform/Main/recovery/physical
qualification operations; and catalog/media publication/inspection. Their
shared legacy bootstrap then keeps the service, protocol crate, startup script
and CI artifact alive. Host-only build/dependency/evidence/USB tools separately
keep the CLI alive, but need not become device-service features. Keeping a
small separately owned legacy tool for those jobs is a valid disposition; this
audit is not a proposal to copy them all into 2.0.

### Validation and review

Seven device parser tests, 187 host tests (including framebuffer views), and
five USB capture tests pass. Focused agent Clippy (`--lib --tests`, warnings
denied) and Rust LSP diagnostics pass. Repository references and source-text
tests were searched explicitly for deleted helpers and analyzer names. Syntax
comparison plus histogram diff confirms retained capture functions changed only
to drop the unused elapsed-time field; retained rendering bodies are untouched.

No deployment, capture, reboot, profile, benchmark or broad local Rust matrix
ran. The mandatory Python pre-push gate and unchanged tooling scope check apply
to the full committed diff; CI owns broader assurance. No 2.0 core path, scope
guard, runtime/application code, manifest or lockfile is modified.

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
