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
the app. Runtime delivery may replace the app independently only by activating
a regenerated manifest with the new app hash in the same rollback-capable
transaction.

There is deliberately no binary-only runtime deployment interface. The app and
manifest are one activation unit even when Main, the scanout module, and the
RBF are unchanged. Main's acknowledgement of `mister_magik_resume` means only
that it accepted the request; successful preflight, child start, latch-backed
ready reporting, and delivery smoke are required before the old unit may be
discarded.

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
writes exactly `ready-v1 token=<token> pid=<supervised-pid>` after the two-post
condition, retrying a temporarily unavailable nonblocking FIFO until Main's
deadline. Main rejects malformed reports and reports from an old child or
spawn. There is no acknowledgement channel, framebuffer-content test, or
route lease.

Main waits eight seconds per attempt. It stops and reaps the first failed child
and retries the complete supervised start once. A second failure stops the
child and restores stable stock Menu. If a display change is provisional,
Main restores the previous timing before enabling stock Menu and does not
restart MagiK. This is internal readiness only and does not prove physical
HDMI or CRT visibility; attended USB-video testing owns that claim.

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
`js*` profiles only for setup, diagnostics, and compatibility with an older
Main that does not advertise `MISTER_MAGIK_INPUT_PROXY=1`.

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

Development delivery first compares the installed manifest with the exact clean
local app commit:

The installed manifest is accepted only when it contains the exact canonical
field set once, uses the fixed paths for its layout, has lowercase hashes, and
contains valid source revisions. Missing, partial, duplicate, extra, or
noncanonical manifests select `Platform`.

- `NoOp` returns after reconciliation when the revisions already match or all
  accumulated app changes are non-deploying.
- `Runtime` verifies the installed platform, builds the app, snapshots the old
  app and manifest, uploads both replacements, activates the manifest last,
  smoke-tests, and rolls both files back on failure inside one typed host
  transaction. It does not reboot.
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

Each mutating transaction uses the exact clean app commit. Platform transactions
stage Main, kernel, and RBF only from the one verified GitHub bundle; none can
be replaced by a local build. Main is never copied directly onto the device
outside the platform transaction.
The host serializes a transaction with a nonblocking process-owned OS lock.
The lock is released automatically when the process exits, including abnormal
exit, and creates no persistent device lease or expiry/recovery state.

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
