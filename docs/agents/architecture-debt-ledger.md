# Architecture containment and P1 migration ledger

This is the current handoff from P0 containment to the P1 Enforce, Decompose,
and Type streams. It records temporary exceptions and edit ownership; it is
not an acceptance target based on file length. The typed checks in
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
| Launch handoff and recovery | `execute_game_launch_with` now builds the portable `LaunchHandoffRequest` and invokes `LaunchIoHandoff`; `SystemLaunchIo` remains the temporary production effect adapter until its raw process, marker, profile, override, and Main calls move behind their owning runtime/persistence capabilities. Recovery remains in `spawn_mister`, `reboot_mister_with`, and `exit_to_mister`. |
| Display control | `LauncherDisplayControl` implements the portable `DisplayControl` capability over typed `MainCommand` calls; the public display helpers retain their existing strings, polling cadence, and transaction-state API as compatibility wrappers. |
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
