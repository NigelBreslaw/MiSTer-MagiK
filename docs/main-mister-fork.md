# External Main_MiSTer Fork

MiSTer MagiK now keeps its Main_MiSTer fork outside this app repo. The normal
checkout layout is:

```text
slint/
  mister-slint/        # Rust/Slint app and deploy tooling
  Main_MiSTer/         # real GitHub fork of MiSTer-devel/Main_MiSTer
```

`scripts/agent deliver` installs one coherent development platform. Main, the
scanout kernel module, and the FPGA latch come from the same latest qualified
GitHub platform release; the tag-addressed verified archive is reused while
that release remains latest. The platform manifest binds those components to
the app. Reconciliation keeps app-only changes in the `Runtime` lane when the
installed Main/platform identity is compatible. That lane builds the runtime
artifact, regenerates the v3 manifest as an inseparable binary/manifest pair,
and activates it through the supervised Dev transaction without rebooting
Linux. Platform identity changes, invalid or incomplete manifests, and Main,
kernel, FPGA, or manager changes remain `Platform` delivery and use the full
manifest-bound transaction with its supervised reboot.

There is deliberately no binary-only runtime deployment interface. The app and
manifest are one activation unit even when Main, the scanout module, and the
RBF are unchanged. Main's acknowledgement of `mister_magik_resume` means only
that it accepted the request; successful preflight, child start, latch-backed
ready reporting, and delivery smoke are required before the old unit may be
discarded. For rapid particle iteration, use the canonical attended
`scripts/agent device scene-lab --scene magik --recipe RECIPE --attended` lane:
it builds only `apps/framebuffer-scene-lab`, runs from volatile `/tmp` state,
and restores Main on exit without a Linux reboot.

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
2. Enters bootstrap black after `video_init()` by disabling LFB routing while
   the MagiK Menu RBF supplies native black pixels with intact timing.
3. Re-enters the same idempotent state before every supervised spawn, runs the
   runtime latch preflight, and transfers FPGA ownership only after it passes.
4. Starts the matching public or development `mister-magik-fb ui launcher 0`
   on `tty2`.
5. Keeps the child in `LauncherStarting` until a token- and PID-bound ready
   report backed by two completed advancing alternating latch posts arrives.
6. Enters dormant launcher mode only after that internal readiness boundary.
7. Polls only launcher lifecycle and explicit handoff commands while Slint owns
   the launcher UI.

Main is the only writer of `UIO_BUT_SW` and the `CONF_VGA_FB` mux bit. Rust
publishes framebuffer geometry and pixels but does not rebuild the framework
configuration word. Bootstrap black uses only the canonical `UIO_SET_FBUF`
disable word and never writes a framebuffer mode or clears `/dev/fb0`. A
bootstrap, preflight, ownership, or spawn failure before the launcher child
exists restores stock Menu input and OSD over the native black background;
those paths remain suppressed for the entire lifetime of a supervised launcher
child.

The one-way ready report uses one mode-0600 FIFO under `/tmp/mister-magik`; it
does not share `/dev/MiSTer_cmd`, whose host operation lock may be held while
waiting for launcher activation. Each spawn receives a new 32-hex token. Rust
writes one canonical `ready-v2` record after the two-post condition. The record
binds the token and supervised PID to Main's PID and generation, the FPGA owner
epoch, latch protocol-v5 identity, route geometry, both advancing alternating
post receipts, and a SHA-256/nonzero-pixel summary of the active RGB565 source
rows. Rust derives that summary from the exact final committed hidden slot,
whether the frame was composed from cached Slint layers or rendered directly
by the startup intro. A temporarily unavailable nonblocking FIFO is retried
until Main's deadline. Main rejects noncanonical fields, stale process or ownership context,
invalid geometry, blank source evidence, and a changed current FPGA owner. This
is source and latch readiness, not proof of sink-visible pixels.

Main waits eight seconds per attempt. It stops and reaps the first failed child
and retries the complete supervised start once. A second failure stops the
child and restores stable stock Menu. If a display change is provisional,
Main restores the previous timing before enabling stock Menu and does not
restart MagiK. Before recovery begins, Main atomically writes a bounded
`mister-magik-return-incident-v1` record with the failed process/ownership
context and `sink_visibility: unobserved`; persistence failure never blocks
stock recovery. This is internal readiness only and does not prove physical
HDMI or CRT visibility; output-rate physical capture owns that claim, while
the 30 fps USB-video path remains supporting evidence only.

When the supervised launcher child exits unexpectedly, the fork records a local
crash report under `/media/fat/mister-magik/crashes/`, updates
`/tmp/mister-magik/main-status.json` with the last report path, and keeps the
existing `LauncherCrashed` recovery path available for
`mister_magik_restart_launcher`.

The generated launcher script invokes `mister-magik-fb library-refresh` only
when that layout's SQLite catalog is missing or empty. The Rust command intentionally
defers that foreground refresh when `MISTER_MAGIK_PARENT` is set, so first boot
after an attended library-data purge reaches Slint immediately and shows the
scan screen. With a
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

Launcher navigation uses Main's stock menu mapping path as well. While the
supervised launcher is active, Main keeps its evdev discovery, controller
quirks, user menu maps, gamecontrollerdb fallback, hot-plug handling, and
custom Menu OK/Back precedence authoritative. Resolved menu actions are emitted
through Main's virtual input device for Rust to consume. Rust retains direct
`js*` profiles only for setup and diagnostics. Production navigation requires
`MISTER_MAGIK_INPUT_PROXY_PROTOCOL=2`; missing or unhealthy v2 input is shown as
a fault and never falls back to raw navigation.

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
./build-container.sh clean all
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

For committed Main-only development experiments, use the permanent positional
workflow:

```bash
scripts/agent deliver local-main
```

It requires clean exact commits in this repository and the sibling
`Main_MiSTer` checkout (`MISTER_MAIN_DIR` may override the latter), runs the
fork state and patch-surface tests, and builds `bin/MiSTer` with
`build-container.sh clean all`. It first verifies the complete installed Dev
platform, then creates an overlay manifest that changes only `main_sha256`,
`main_revision`, and `qualification_candidate_id`. The typed transaction
snapshots Dev Main and the manifest, uploads Main first, activates the manifest
last, and retains rollback until the running `/proc/<pid>/exe`, the complete
installed platform, and the launcher smoke test all pass. The app, manager,
scanout module, and latch RBF are never built or replaced by this workflow.
The recorded app revision is the verified installed MagiK revision, which may
legitimately predate a clean host-only tooling commit at repository HEAD.

An installed Main that advertises supervised local reload is replaced without
rebooting Linux. The initial commit that introduces that capability necessarily
uses one bounded Linux reboot. A rollback uses supervised replacement where
possible and at most one bounded recovery reboot when the failed Main can no
longer reload itself. Ordinary `scripts/agent deliver` remains the canonical
way to restore the latest published platform.

Development delivery first compares the installed manifest with the exact clean
local app commit:

The installed manifest is accepted only when it contains the exact canonical
field set once, uses the fixed paths for its layout, has lowercase hashes, and
contains valid source revisions. Missing, partial, duplicate, extra, or
noncanonical manifests select `Platform`.

- `NoOp` returns after reconciliation when the revisions already match or all
  accumulated app changes are non-deploying.
- `Runtime` builds and activates the Dev runtime/manifest pair through the
  no-reboot coherent transaction. It skips GitHub platform resolution, manager
  qualification, database preparation, and the reboot phase.
- `Platform` qualifies and activates the complete Main, app, kernel, FPGA,
  manager, database-support, and manifest set with reboot and rollback
  protection inside one typed host transaction.

Component receipts and release archives under `build/agent-cache/` bind
immutable source, build, toolchain, compiler, Dockerfile, OCI image,
configuration, and artifact identities. Old receipt versions are cache misses.
The complete published platform archive and game-database artifacts are reused
across deliveries; cleanup removes only disposable `build/agent-deploy/`
staging.
An unchanged manager is fetched only through the typed host API after the
installed manifest and remote checksum have been verified, then cached by
SHA-256. Changed manager inputs or failed installed verification use a strict
local build receipt instead.

Each mutating transaction uses the exact clean app commit. Canonical platform
transactions stage Main, kernel, and RBF only from the one verified GitHub
bundle. The only local Main exception is the fixed Dev-only
`deliver local-main` Main/manifest transaction above; no generic upload or local
module/RBF route exists.
The host serializes a transaction with a nonblocking process-owned OS lock.
The lock is released automatically when the process exits, including abnormal
exit. Local-Main delivery additionally owns a durable device-side transaction
marker and rollback pair: a later invocation reconciles an interrupted swap
before verifying or snapshotting the installed platform.

The development script deploys:

- `apps/mister` to `/media/fat/mister-magik-dev/mister-magik-fb`
- the bundle Main to `/media/fat/MiSTer_MagiKDev`
- the bundle scanout module and metadata to `/media/fat/mister-magik-dev/`
- the bundle Menu latch RBF and metadata to `/media/fat/mister-magik-dev/fpga/`
- the complete platform contract to `/media/fat/mister-magik-dev/platform-v3.manifest`

The transaction selects the development Main only after the complete manifest
has been activated.

The manifest is activated last. The deploy script never writes root
`/media/fat/menu.rbf`, which remains owned by `update_all`.
That stock path and its compatibility redirect must not be used to activate or
verify an experimental MagiK RBF. The typed transaction sends Main-owned
`load_core` with the exact layout-selected `magik_latch_menu_path()`. The
`mister_magik_reload_main` command only replaces Dev Main; its absolute RBF
argument is not handled by the startup auto-loader and therefore is not FPGA
activation evidence.

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
6. Deploy from `mister-slint`; delivery resolves and reuses the latest published
   platform bundle.
7. Record device smoke results in the fork `MAGIK_PATCHSET.md`.

## Historical Notes

The embedded `main-mister/` directory in this app repo was the old experiment
location. It is no longer the maintained source. Historical audits and device
results remain under `history/`, especially:

- `history/2026-6-14/main-mister-clean-reset-audit.md`

Older notes may mention `main-mister/`; read those as history unless the current
workflow above says otherwise.
