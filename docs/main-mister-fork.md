# External Main_MiSTer Fork

MiSTer MagiK now keeps its Main_MiSTer fork outside this app repo. The normal
checkout layout is:

```text
slint/
  mister-slint/        # Rust/Slint app and deploy tooling
  Main_MiSTer/         # real GitHub fork of MiSTer-devel/Main_MiSTer
```

`scripts/deploy-main-mister-experiment.sh` defaults to
`../Main_MiSTer`. Set `MISTER_MAIN_DIR` when the fork lives elsewhere.

The fork is not a submodule. It has its own history, CI, build wrapper, and
patch ledger.

## Fork Source Of Truth

The maintained fork repo is `NigelBreslaw/Main_MiSTer`, a real GitHub fork of
`MiSTer-devel/Main_MiSTer`. The long-lived MiSTer MagiK branch is
`mister-magik`.

- Upstream project: `MiSTer-devel/Main_MiSTer`
- Baseline commit: `c73802332ff9c73659410084b6319ccd29f0b3aa`
- Baseline release: `Release 20260603.`
- Device binary: `/media/fat/MiSTer_MagiK`
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

The fork then:

1. Initializes video/menu-core prerequisites.
2. Runs Rust `early-black` after `video_init()` so Rust owns the launcher
   framebuffer mode and scan-out route.
3. Starts `/media/fat/mister-magik/mister-magik-fb ui launcher 0` on `tty2`.
4. Enters dormant launcher mode.
5. Polls only launcher lifecycle and explicit handoff commands while Slint owns
   the launcher UI.

The explicit command surface is:

```text
mister_magik_launch <absolute .mgl/.mra/.rbf path>
mister_magik_exit_to_menu
```

Commands are valid only from `LauncherActive`. Launch shuts down Slint and uses
Main's normal loader path. Exit shuts down Slint and restores the stock Main
menu path.

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
./build-docker.sh
scripts/test-magik-state.sh
scripts/check-magik-patch-surface.sh
```

Deploy from this app repo:

```bash
scripts/deploy-main-mister-experiment.sh
```

Use a non-default fork checkout:

```bash
export MISTER_MAIN_DIR=/path/to/Main_MiSTer
scripts/deploy-main-mister-experiment.sh
```

The deploy script no longer forces a clean Main build. Use `--clean-main` only
when stale Main objects are suspected:

```bash
scripts/deploy-main-mister-experiment.sh --clean-main
```

The script deploys:

- `magik-gui` to `/media/fat/mister-magik/mister-magik-fb`
- `$MISTER_MAIN_DIR/bin/MiSTer` to `/media/fat/MiSTer_MagiK`
- stock inittab plus `[MiSTer] main=MiSTer_MagiK`

## Sequential Release Process

Do not stack branches.

1. Merge each fork repo PR to `Main_MiSTer/mister-magik`.
2. Run the fork host tests, patch-surface check, and Docker build.
3. Merge app repo deploy/docs changes to `mister-slint/main`.
4. Deploy from `mister-slint` with the fork checkout available at
   `../Main_MiSTer` or `MISTER_MAIN_DIR`.
5. Record device smoke results in the fork `MAGIK_PATCHSET.md`.

## Historical Notes

The embedded `main-mister/` directory in this app repo was the old experiment
location. It is no longer the maintained source. Historical audits and device
results remain under `history/`, especially:

- `history/2026-6-14/main-mister-clean-reset-audit.md`
- `history/2026-6-3/zaparoo-fork-surface.md`

Older notes may mention `main-mister/`; read those as history unless the current
workflow above says otherwise.
