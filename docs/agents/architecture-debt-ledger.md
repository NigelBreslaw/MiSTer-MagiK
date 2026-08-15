# Architecture boundary and remaining P1 ownership ledger

This is the current handoff from completed P1 Enforce work to the remaining
Decompose and Type streams. It records enforced boundaries and edit ownership;
it is not an acceptance target based on file length. The typed checks in
`agent-cli/src/checks.rs` and their selection in `agent-cli/src/planner.rs`
remain the executable containment boundary.

## P0 qualification state

The P0 implementation commits are on the current branch, and each planned
commit passed its fast gate. P0 is not formally complete until the full
affected pre-push assurance and CI have passed for this branch. Do not use this
ledger to waive that final condition.

## Main command endpoint inventory

The FIFO check permits exactly these paths. Moving a capability requires
changing the implementation and this exact inventory together; adding another
path is not a migration strategy.

| Current path | Capability owner | Destination and removal condition |
|---|---|---|
| `mister/platform/runtime/src/main_command.rs` | P1 Enforce | Sole production-app command transport. Keep wire spelling, serialized reply association, bounded FIFO writes, and endpoint paths private to this typed module. |
| `mister/tools/agent/src/main.rs` | P1 Enforce device-service boundary | This is a distinct attended device-service capability, not production app transport. Keep the entry only while this file performs that service operation; if it moves, update the inventory atomically after zero old references. |
| `agent-cli/src/host/remote.rs` | P1 Enforce host boundary | This constructs bounded host commands and is not production app transport. Keep the entry only while this file owns that construction; if it moves, update the inventory atomically after zero old references. |

The destructive fault exception is closed. Catalog and app persistence expose
only `DirectResetFaultControl` event evidence; platform runtime owns volatile
arming, marker/session paths, the typed no-reply Main reset command, bounded
delay, and the stable seven-artifact cleanup operation.

## Launcher platform-effect characterization

P1 Enforce begins from these direct effect families in
`apps/mister/src/launcher.rs`. The named production symbols and their observable
ordering are characterization inputs; moving a row must preserve its behavior
or record an explicit behavior change.

| Capability | Current direct effect owners |
|---|---|
| Main command transport | `mister/platform/runtime/src/main_command.rs`; launcher callers use typed `MainCommand` variants and have no endpoint access |
| Launch handoff and recovery | `execute_game_launch_with` builds the portable `LaunchHandoffRequest` and invokes `LaunchIoHandoff`; `SystemLaunchIo` composes platform-owned Main/runtime adapters with launcher-owned input profiles, button overrides, and recovery ordering. Recovery remains in `spawn_mister`, `reboot_mister_with`, and `exit_to_mister`. |
| Display control | Platform runtime's `MainDisplayControl` implements the portable `DisplayControl` capability over typed `MainCommand` calls and owns response parsing; the public launcher display helpers retain their existing strings, polling cadence, and transaction-state API as compatibility wrappers. |
| Runtime state and process inspection | Platform runtime's `SystemRuntimeState` implements the portable `RuntimeState` snapshot for registered Main process state, arcade-core command-line classification, and heartbeat; launcher compatibility helpers now delegate to that capability. |
| Launcher persistence | `SystemLauncherPersistence` implements the portable `LauncherPersistence` capability for launch-return state, settings, input policy, and rebuild-marker ownership; launcher-facing helpers remain compatibility wrappers. Button profiles, screenshot-pack cleanup, and menu-wallpaper restoration remain explicit launcher composition effects. |

The launch fake freezes preparation, override, marker, command, and recovery
ordering. Main reply association remains serialized by the command-operation
lock and has no request identifiers or absolute reply deadline: it waits while
Main is alive and its heartbeat advances, and fails on channel closure,
oversized/malformed reply, process exit, or stopped heartbeat.

The unused broad `MagikPlatform`, `MisterRuntimeBackend`, and `MisterRuntime`
abstractions were removed after semantic reference checks proved that only
their declarations, adapter implementation, and isolated fake remained.

## Runtime configuration containment

`apps/mister/config/runtime-environment.toml` is the sole registry for the 272
currently owned names and its explicit source roots. Its generated reference is
`docs/reference/mister-runtime-environment.md`. Registration is a temporary
growth boundary, not approval for new downstream reads.

P1 Type owns the schema expansion, process-boundary capture, typed subconfigs,
and removal of downstream direct reads. The registry and generated reference
remain after migration. The P0 baseline counts and legacy read tolerance may be
removed only when each process has one named parse site, registered aliases and
external inputs remain explicit, and the negative fixtures reject downstream
reads. No other P1 stream may create a competing registry or config vocabulary.

The application process boundary now captures one immutable environment
snapshot after command resolution. Its typed launcher-readiness section owns
the startup token, ready FIFO, Main PID/generation, and owner epoch; the
diagnostic section captures the readiness-report JSON modifier. The existing
`ready-v2` state machine consumes that typed section without changing token
acceptance, posted-frame evidence, retry, or send behavior. Compatible fault
configuration is also derived from the same snapshot through the catalog-owned
redacting parser. Remaining registered controls retain their temporary direct
read sites until their named Type-A migration batches land.

## Platform-v3 authority

`mister/platform/contracts/platform-v3.schema.toml` is the canonical neutral
descriptor and `mister/platform/contracts/manifest` is the behavioral parser,
serializer, validation-profile, installed-layout, and candidate-identity
authority. `agent-cli/src/platform_manifest.rs` retains only agent-owned
artifact hashing and file orchestration around that contract.

Checked-in shell constants and byte-stable public/development fixtures under
`mister/platform/contracts/generated/` are generated from the descriptor by
`scripts/checks/generate-platform-v3-consumers.py`. The typed repository check
requires exact generated constants, strict round-trip fixture validity, and no
structural duplicate outside the contract. The installer, packaging, release
fixture, distribution workflow, and embedded-catalog release fixture now
consume or check those outputs; the temporary legacy-consumer ledger and
agent behavioral-authority designation are closed.

`mister/tools/manager/src/main.rs` now delegates manifest structure, public
layout, and legacy-compatible identity validation to the shared contract's
`ManagerLegacy` profile while retaining manager-owned filesystem hashing,
metadata checks, and error presentation. All migrated validation profiles now
recompute the qualification candidate identity and reject forged lower-case
hex values. The GUI profile also rejects both missing and additional fields,
using the schema-generated exact platform-v3 field set. GUI identity validation
hashes all seven installed artifacts: the running GUI executable plus Main,
manager, scanout module and metadata, and latch RBF and metadata.

Public and development installed roots remain distinct. Platform-v3 bytes,
field order, and accepted installed layouts are unchanged; video diagnostics
and HDMI evidence remain separate contracts.

`crates/catalog/src/device_layout.rs` now preserves its existing public API and
executable-relative selection semantics while sourcing both installed roots,
Main paths, and component paths from the shared generated layout contract.

The agent host now maps its public/development transport layout to the shared
generated roots and component paths. Host-only subpaths are traversal-checked
relative paths, while `/tmp` operation state remains host-owned. This includes
platform deployment, attended catalog purge, readiness and return
qualification, diagnostics, and platform-repair safety paths; manifest
fixtures in deploy and local-Main delivery are generated from the same typed
contract.

The GUI resolves its manifest, media, diagnostics, settings, and launcher-owned
files through the catalog layout adapter, which now delegates to the shared
contract. The last unused public-layout video constants were removed; public
and Dev runtime selection continues to follow the executable location.

The manager now derives its public app root, manifest path, component fixtures,
and legacy inittab removal prefixes from the shared public layout while
preserving `MISTER_MAGIK_FAT` test/install-root remapping.

## Device crate-root migration inventory

The device application has zero shared source owners compiled through both the
library and binary roots. The static device crate-root ownership check rejects
any new bin/lib redeclaration. Migration is library-authoritative: the binary
imports shared owners through `mister_magik_fb::...`; it must not become an
import authority for the library.

The inventory was ratcheted after each batch. Runtime modules are migrated only
after typed process configuration is captured and
passed through the existing composition root. `ui_runner` remains an
ownership-only move: readiness, scheduling, frame phases, presentation, and
the `ready-v2` posted-frame contract do not change in this track.

Completed: `20a-reporting-identity` moved `artifact_publish`, catalog/latch
reports, `diagnostic_identity`, and `fallible_log`; `20b-media` moved
`media_http`, `media_pack_save`, and `video_i420`; `20c-rendering-display`
moved `arcade_list_renderer`, `bitmap_text`, and `ui_display` to library
authority; `20d-cfg-test` moved `experiments/effects` and `test_support` to
library authority.

Runtime batch `21a` moved device input, input-hub/integrity, display detection,
and frame-profile modules behind the library's existing `ui` feature. Binary
children retain their `crate::...` paths through root imports; the source files
now compile under only the library namespace.

Runtime batch `21b` moved CPU/PMU profiling, preview-pack benchmark, and search
benchmark support behind library feature cfgs. `ui_effect_bench` remains in the
binary until `21d` because it imports `ui_runner` platform types; moving it
earlier would invert the intended library ownership direction.

Runtime batch `21c` moved media benchmarks, preview state/transitions, and the
ARM video/audio runtime behind library cfgs. The transition experiment leaf
also moved under the library's existing experiments namespace so
`screenshot_transitions` retains its internal import direction.

Runtime batch `21d` moved allocation/memory support, `ui_runner`, and the
deferred effect benchmark behind library cfgs. The binary now declares no
application source modules. Its former inline tests moved with `app_entry` in
the following extraction. This was an ownership-only move: launcher frame phases, scheduling,
presentation, readiness, and posted-frame evidence were not decomposed or
redesigned.

`app_entry::run` now owns monotonic process state, CPU-profile startup, argument
capture, panic/identity analytics, command resolution, locks, device effects,
dispatch, and shutdown behavior. The binary bootstrap retains the allocator
and the required first-operation installed-layout initialization before calling
the library entrypoint. The characterized startup order is unchanged. The
crate-root check also enforces that narrow binary shape and rejects any new
module declaration or command/config parsing in `main.rs`.

The former application-wide dead-code allowance was removed with the entrypoint
move. The crate-root check rejects its return in either executable edge. The
remaining leaf `#[allow(dead_code)]` annotations in `lib.rs` are cfg-local to
shared rendering helpers that are intentionally partial in host/preview graphs.
The `app_entry`, `cpu_profile`, `input_hub`, `preview_state`, and `ui_runner`
roots use module-scoped dead-code allowances for tests and non-device host
targets because those builds deliberately compile partial UI graphs. Linux/ARM
production builds retain full dead-code enforcement.
The full feature matrix also proves `app_entry` imports only library modules it
uses under the selected cfg, while typed ready-FIFO capture retains the legacy
empty `PathBuf` fallback. Feature-specific application tests now target that
owning library root instead of rebuilding the retired binary module graph.

## Executable failure boundary

Agent protocol v2 retains its legacy `error` string and framing. An optional
`failure` sibling now carries stable code, detail, phase, retry policy, and
recovery-required fields. Protocol parsing preserves unknown future enum values
and falls back to the legacy string when the sibling is absent or malformed.
Host classification, device family emission, and CLI evidence propagation are
ratcheted in the following commits; the wire addition alone changes no exit
code or human-facing first line.

The host agent client now prefers valid structured metadata and falls back to
the legacy string. `AgentError` retains the full wire classification—including
unknown future values, phase, retry policy, and recovery flag—while exposing a
compatible `DeviceFailure` mapping and keeping the legacy error as its display
text. The metadata payload is boxed internally so the typed error remains below
the repository's small-error lint without changing its accessors or semantics.

Device emission is ratcheted by family. Unknown commands, empty/malformed
requests, missing commands, and oversized/non-UTF-8 request headers now emit
`unknown_command` or `invalid_request` metadata while retaining their exact
legacy error strings.

All authenticated control and binary-stream endpoints now emit
`authentication_required` metadata for the existing `unauthorized` response.
Authentication text, logging, and connection behavior are unchanged; no token
value enters the structured sibling.

Control saturation, exclusive framebuffer-consumer contention, producer
connection failure, and control transport setup/read failure now emit
`device_busy` or `device_unavailable` metadata in the `availability` phase.
Their legacy strings and connection behavior are unchanged, and both families
advertise only the existing bounded retry behavior.

Ordinary command, capture, snapshot, preview, launcher-automation, and reboot
failures now emit `operation_failed` metadata in the `operation` phase. Their
legacy strings remain authoritative for humans, and the metadata deliberately
does not authorize blind replay of either read or mutation requests.

The alpha-candidate transaction now retains typed validation, operation,
artifact-verification, and configuration-recovery outcomes until device
emission. Hash mismatches require reconciliation before retry. Only a failed
Downloader configuration restore emits `recovery_required=true` with an
operator-required retry policy; transaction order and legacy text are
unchanged.

Its attended public-layout verification also derives the manifest and complete
component set from `Layout::Public`; no device-agent copy of platform-v3
installed paths remains.

The repository-aware CLI executable and context constructor now retain
`AgentResult` through the reporting boundary, and reporter/evidence storage
errors convert there without an intermediate string-returning `run`. Fatal and
request-failure first-line text and all exit-code decisions remain unchanged;
the next projection step may therefore record structured metadata without
reconstructing it from display strings.

Failed progress events and SQLite evidence now retain a redacted structured
projection (`code`, `phase`, `retry_policy`, and `recovery_required`) through
nested `AgentError` phases. Human message rendering is unchanged. The nullable
v12 evidence column leaves old records compatible and intentionally avoids a
second structured copy of device detail or credentials.

## Enforce closure

The executable-boundary builtin now requires the portable capability surface,
platform-owned Main/display/runtime/fault adapters, launcher-owned handoff and
persistence composition, structured device emission, `AgentResult` CLI edge,
and redacted evidence projection. Separate negative guards reject every former
P1 bypass: unowned direct FIFO access, copied platform-v3 structure or installed
component paths, duplicate binary/library module roots, and string-flattened
device or CLI failure edges. There is no temporary P1 Enforce exception or P0
error-boundary exception left in this ledger; the remaining temporary runtime
environment reads belong only to the separately sequenced Type lane.

## Hotspot ownership

The PR advisory report uses stable owner IDs so moves do not erase history.
These destinations govern the next work; line-count reduction alone does not.

| Owner ID and current source | Next owner and intended seam |
|---|---|
| `launcher-runtime` — `apps/mister/src/ui_runner/launcher_loop.rs` | P1 Decompose: explicit launcher state, frame phases, effects, composition, and presentation. |
| `host-workflows` — `agent-cli/src/host/mod.rs` | P2-A: typed host workflow modules after structured failures exist. |
| `desktop-app` — `apps/desktop/src/main.rs` | P2 next-tier consolidation: desktop ownership seams. |
| `catalog-persistence` — `crates/catalog/src/sqlite_catalog.rs` | P2-B characterization/decomposition, then P3 persistence separation. |

## P1 concurrency and exclusive seams

Enforce, Decompose, and Type may use separate worktrees concurrently only when
their active commits do not touch a shared seam below. Changes crossing a seam
are serialized in this order:

- The compatible `FaultConfig` parsing and arming-policy capture landed before
  Enforce moves the command effect from `crates/catalog/src/fs_fault.rs`.
- Enforce owns stable installed roots and component paths. Its adoption of
  `crates/catalog/src/device_layout.rs` precedes Type's catalog, cache,
  temporary-path, and process-override derivation; the file is never edited by
  both streams concurrently.
- Enforce establishes one app module-root authority and migrates launcher
  platform effects before deep Decompose work. Type supplies typed process
  configuration before Decompose assembles `LauncherRuntime`.
- Type's typed presenters precede Decompose projection work that consumes
  them. Until then, Decompose uses the existing presenter without editing the
  bridge concurrently.

The following are no-concurrent-edit seams regardless of worktree separation:

- `apps/mister/src/main.rs`
- `apps/mister/src/app_entry.rs`
- `apps/mister/src/lib.rs`
- `apps/mister/src/launcher.rs`
- `apps/mister/src/ui_runner/launcher_loop.rs`
- `apps/mister/src/ui_runner/launcher_bridge.rs`
- `crates/catalog/src/device_layout.rs`
- `agent-cli/src/checks.rs`
- `agent-cli/src/planner.rs`

Before starting a P1 commit, choose its narrowest row in
`docs/agents/task-map.md`, confirm the exception or seam still exists, and use
`scripts/agent plan` to preview the affected assurance. If ownership has moved,
update this ledger in the owning migration commit rather than following stale
path assumptions.
