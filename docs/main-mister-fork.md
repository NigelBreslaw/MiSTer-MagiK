# External Main_MiSTer Fork

MiSTer MagiK now keeps its Main_MiSTer fork outside this app repo. The normal
checkout layout is:

```text
slint/
  mister-slint/        # Rust/Slint app and deploy tooling
  Main_MiSTer/         # real GitHub fork of MiSTer-devel/Main_MiSTer
```

`scripts/agent deliver` infers platform impact and uses
`../Main_MiSTer`. Set `MISTER_MAIN_DIR` when the fork lives elsewhere.

The fork is not a submodule. It has its own history, CI, build wrapper, and
patch ledger.

Production Main is also an independently built platform component. The fork
creates `MiSTer_MagiK`, `main-component-v0.1.json`, and `SHA256SUMS`; the receipt
binds the authoritative repository and branch, exact 40-character revision,
pinned toolchain, binary size/hash, and deterministic component ID. MagiK CI
verifies that contract independently.

## Fork Source Of Truth

The maintained fork repo is `NigelBreslaw/Main_MiSTer`, a real GitHub fork of
`MiSTer-devel/Main_MiSTer`. The long-lived MiSTer MagiK branch is
`mister-magik`.

- Upstream project: `MiSTer-devel/Main_MiSTer`
- Baseline commit: `93d13fb690db4581768389450fb639822ae88333`
- Baseline release: `Release 20260707.`
- Public device binary: `/media/fat/MiSTer_MagiK`
- Development device binary: `/media/fat/MiSTer_MagiKDev`
- Ledger: `MAGIK_PATCHSET.md` in the fork repo
- Provenance doc: `FORK.md` in the fork repo

`MAGIK_PATCHSET.md` is the rebuild contract. It lists the intended features,
approved patch surface, implemented tests, and rebuild-from-scratch checklist.
If upstream changes massively, rebuild from the upstream release commit and
reapply only the ledgered MagiK features.

## Ownership Model

The fork is a full Main_MiSTer binary because Main must initialize HDMI/video
before Slint can produce a visible Linux framebuffer UI.

Production boot still starts stock `/media/fat/MiSTer` from `/etc/inittab`.
Stock Main reads `MiSTer.ini`, then `[MiSTer] main=MiSTer_MagiK` re-execs the
fork.

The fork selects its application root from its executable name, then:

1. Initializes video/menu-core prerequisites.
2. Runs Rust `early-black` after `video_init()` so Rust owns the launcher
   framebuffer mode and scan-out route.
3. Starts the matching public or development `mister-magik-fb ui launcher 0`
   on `tty2`.
4. Enters dormant launcher mode.
5. Polls only launcher lifecycle and explicit handoff commands while Slint owns
   the launcher UI.

When the supervised launcher child exits unexpectedly, the fork records a local
crash report under `/media/fat/mister-magik/crashes/`, updates
`/tmp/mister-magik/main-status.json` with the last report path, and keeps the
existing `LauncherCrashed` recovery path available for
`mister_magik_restart_launcher`.

The generated launcher script invokes `mister-magik-fb library-refresh` only
when that layout's SQLite catalog is missing or empty. The Rust command intentionally
defers that foreground refresh when `MISTER_MAGIK_PARENT` is set, so first boot
and Reset Database reach Slint immediately and show the scan screen. With a
usable catalog present, the Rust launcher background stamp check is the only
normal boot-time validation owner; Main must not schedule a delayed external
refresh.

The explicit command surface is:

```text
mister_magik_launch <absolute .mgl/.mra/.rbf path>
mister_magik_launch_plan_v1 <encoded structured catalog plan>
mister_magik_exit_to_menu
```

Commands are valid only from `LauncherActive`. Launch shuts down Slint and uses
Main's normal loader path. Exit shuts down Slint but remains on the active
manifest-qualified latch Menu core; it does not reload root `menu.rbf`.

Structured catalog plans are a MagiK-only handoff path for virtual
`magik-plan:*` launcher rows. Rust sends `schema=1`, `core_path`,
`payload_path`, `mount_kind`, `mount_index`, `delay_secs`, title/system
metadata, and `launch_ref`; Main resolves `core_path` with MRA-style RBF
semantics, carries the encoded plan through re-exec as `magik-plan-v1:...`, and
seeds the existing MGL action state directly in `user_io_init`. Real user `.mra`,
`.mgl`, and `.rbf` paths stay on `mister_magik_launch`.

Simple joystick handling is also launch-scoped. Rust owns the persistent setting
and the arcade policy: when the setting is enabled for a direct `.mra` launch,
MagiK parses the MRA button labels before handoff and writes
`/tmp/mister-magik/button-overrides` with zero-based MRA button indexes mapped
to virtual Main tokens or `unmap`. Main only checks the boot-local
`/tmp/mister-magik/input-policy` marker, skips user/core input maps in simple
mode, and applies the override file as a generic adapter while constructing its
normal input maps. Main must not parse MRA XML or classify arcade labels such as
coin, start, pause, service, or player-two controls.

Runtime display transactions preserve automatic sink detection only while
applying `auto`. Every explicit HDMI or CRT mode clears Main's automatic-routing
latch before loading geometry, so cancel and timeout restore the saved route
instead of re-resolving the previous automatic route against the current sink.

## Defensive Diagnostics

The clean model is not "clever suppression" as normal operation. Main should not
attempt OSD, framebuffer routing, framebuffer mode writes, or menu-background
creation while Slint owns the launcher.

The fork still has narrow defensive invariant diagnostics at those entrypoints.
If they fire, the status/event files under `/tmp/mister-magik/` should show an
unexpected event. Idle launcher operation should not produce those events.

## Build And Deploy

Build the fork directly from the fork repo:

```bash
cd ../Main_MiSTer
./build-container.sh
scripts/test-magik-state.sh
scripts/check-magik-patch-surface.sh
```

The current approved patch surface includes the narrow
`support/arcade/mra_loader.cpp` / `.h` helper extraction needed for structured
handoff: shared RBF-name resolution and direct MGL action seeding. Keep the
protocol details and device smoke results current in the fork's
`MAGIK_PATCHSET.md`; that file is the rebuild ledger.

Deploy from this app repo:

```bash
scripts/agent deliver
```

When GitHub cannot publish a new platform candidate, a committed and pushed
Main-only change can be delivered transactionally from the local fork:

```bash
scripts/agent deliver --local-main
```

This development-only escape hatch builds and checks the clean
`mister-magik` branch, reuses the latest published candidate's unchanged
FPGA and kernel artifacts, substitutes the locally built Main, regenerates the
complete development manifest, and uses the normal snapshot, activation,
reboot, smoke, and rollback transaction. It does not publish a platform release
and never copies Main directly onto the device. App-repository platform changes
remain blocked until their exact candidate is published.

Use a non-default fork checkout with the same option:

```bash
export MISTER_MAIN_DIR=/path/to/Main_MiSTer
scripts/agent deliver --local-main
```

The development script deploys:

- `apps/mister` to `/media/fat/mister-magik-dev/mister-magik-fb`
- `$MISTER_MAIN_DIR/bin/MiSTer` to `/media/fat/MiSTer_MagiKDev`
- the qualified scanout module and metadata to `/media/fat/mister-magik-dev/`
- the qualified Menu latch RBF and metadata to `/media/fat/mister-magik-dev/fpga/`
- the complete platform contract to `/media/fat/mister-magik-dev/platform-v2.manifest`

It does not change the selected Main. Activate it with `mister mode dev` after
the complete manifest verifies.

The manifest is activated last. The deploy script never writes root
`/media/fat/menu.rbf`, which remains owned by `update_all`.

## Sequential Release Process

Do not stack branches.

1. Merge each fork repo PR to `Main_MiSTer/mister-magik`.
2. Run the fork host tests and patch-surface check. The unified platform
   workflow reuses Main from the latest platform release when its identity is
   unchanged and builds it with the pinned toolchain only when it changed.
3. Merge app repo deploy/docs changes to `mister-slint/main`.
4. Run **Build MiSTer MagiK Platform**. It captures the fork head once, reuses
   unchanged components from the latest release, and builds only changed
   components. Approve immutable publication only after candidate verification.
5. Application publication consumes Main from the highest v0.2 platform bundle;
   only a legacy v0.1 bundle invokes the temporary source-build fallback.
6. Deploy from `mister-slint` with the fork checkout available at
   `../Main_MiSTer` or `MISTER_MAIN_DIR`.
7. Record device smoke results in the fork `MAGIK_PATCHSET.md`.

## Historical Notes

The embedded `main-mister/` directory in this app repo was the old experiment
location. It is no longer the maintained source. Historical audits and device
results remain under `history/`, especially:

- `history/2026-6-14/main-mister-clean-reset-audit.md`

Older notes may mention `main-mister/`; read those as history unless the current
workflow above says otherwise.
