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
| `crates/catalog/src/fs_fault.rs` | P1 Type then P1 Enforce, serially | Type first captures compatible `FaultConfig` parsing and arming policy. Enforce then injects and moves command transport. Remove the exception after the typed config feeds the runtime-owned effect and the catalog file performs no endpoint I/O. |
| `mister/tools/agent/src/main.rs` | P1 Enforce device-service boundary | This is a distinct attended device-service capability, not production app transport. Keep the entry only while this file performs that service operation; if it moves, update the inventory atomically after zero old references. |
| `agent-cli/src/host/remote.rs` | P1 Enforce host boundary | This constructs bounded host commands and is not production app transport. Keep the entry only while this file owns that construction; if it moves, update the inventory atomically after zero old references. |

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
| Runtime state and process inspection | `main_heartbeat`, `mister_running`, `mister_running_arcade_core` |
| Launcher persistence | launch-return state helpers, input-policy/button-profile helpers, rebuild-marker helpers, screenshot-pack cleanup, and menu-wallpaper restoration |

The launch fake freezes preparation, override, marker, command, and recovery
ordering. Main reply association remains serialized by the command-operation
lock and has no request identifiers or absolute reply deadline: it waits while
Main is alive and its heartbeat advances, and fails on channel closure,
oversized/malformed reply, process exit, or stopped heartbeat.

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

## Platform-v3 legacy consumers

`mister/platform/contracts/platform-v3.schema.toml` is the neutral structural
descriptor. `agent-cli/src/platform_manifest.rs` is the temporary behavioral
parser and serializer authority. The following legacy entries are exact; P1
Enforce removes each entry only when its consumer adopts the shared
`platform-manifest-contract` authority or a generated consumer from the same
descriptor.

The additive `mister/platform/contracts/manifest` leaf crate generates its
constants from the descriptor at build time. Its presence does not switch
behavioral authority: that transition remains atomic with agent adoption after
fixture and rejection-class parity is proven.

`agent-cli/src/platform_manifest.rs` now delegates structural parsing,
serialization, layout paths, and candidate identity to that crate while
retaining artifact hashing and file orchestration. The descriptor continues to
name this thin module as the compatibility-window behavioral authority until
the remaining named consumers move; its direct host-file ledger entries are
not removed by this core adoption alone.

| Legacy consumer | Migration owner |
|---|---|
| `agent-cli/src/host/mod.rs` | Enforce contract and agent adoption |
| `agent-cli/src/host/platform_deploy.rs` | Enforce contract and agent adoption |
| `agent-cli/src/deploy.rs` | Enforce contract and agent adoption |
| `agent-cli/src/local_main_delivery.rs` | Enforce contract and agent adoption |
| `mister/tools/manager/src/main.rs` | Enforce manager adoption |
| `crates/catalog/src/device_layout.rs` | Enforce installed-layout adoption, before Type path overrides |
| `scripts/MiSTer-MagiK.sh` | Enforce generated non-Rust consumers |
| `scripts/package-distribution.sh` | Enforce generated non-Rust consumers |
| `scripts/release/check-host.sh` | Enforce generated non-Rust consumers |
| `.github/workflows/distribution.yml` | Enforce generated non-Rust consumers |
| `scripts/tests/test-embedded-catalog-release.py` | Enforce generated non-Rust fixtures |

The descriptor remains canonical after the migration. Remove the temporary
`agent-cli` behavioral-authority designation only after byte-identical public
and development fixtures, all current rejection classes, and every named
consumer have moved. Public and development installed roots must remain
distinct throughout.

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

- Type owns `FaultConfig` parsing and arming policy in
  `crates/catalog/src/fs_fault.rs` before Enforce moves its command effect.
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
