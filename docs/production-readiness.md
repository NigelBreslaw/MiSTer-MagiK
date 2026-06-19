# Production Readiness

This is the public-beta release gate for MiSTer MagiK. It is intentionally
stricter than everyday development and assumes real MiSTer hardware remains a
manual pre-release gate rather than GitHub-hosted CI.

## Release Criteria

- Host checks pass with `scripts/release-check-host.sh`.
- The external Main_MiSTer fork passes its own gate from `../Main_MiSTer`:
  `scripts/test-magik-state.sh`, `scripts/check-magik-patch-surface.sh`, and
  `./build-container.sh`.
- Device acceptance passes with `scripts/device-release-acceptance.sh --deploy`.
- The distribution zip contains the installer script, `mister-magik-fb`,
  `mame.sqlite3`, and `MiSTer_MagiK` when the Main fork binary is supplied.
- Rollback is verified with `scripts/restore-stock-boot.sh` before publishing a
  beta build from a new release flow.

## Host Gate

Run:

```bash
scripts/release-check-host.sh
```

This runs formatting, host logic tests, catalog crate tests, host-tool tests,
clippy, the ARM release build, shared-library validation, and a package dry run.
It writes disposable package-check artifacts under `build/release-check-host/`.

GitHub Actions remains host/build-only. The workflow explicitly runs the catalog
crate tests so catalog behavior is not only checked indirectly through the app
crate.

## Device Gate

Run against the currently deployed build:

```bash
scripts/device-release-acceptance.sh --skip-deploy
```

Run the full release rehearsal:

```bash
scripts/device-release-acceptance.sh --deploy
```

Run a quick non-destructive smoke while iterating on the deployed build:

```bash
scripts/device-release-acceptance.sh --skip-deploy --fast
```

Run the long soak only when explicitly requested:

```bash
scripts/device-release-acceptance.sh --skip-deploy --soak
```

The script uses only `scripts/mister` for device access and writes artifacts to
`build/device-release/<timestamp>/`, including status JSON, doctor output,
snapshots, logs, optional frame/profile files, and `report.md`.

The device gate is telemetry-first: it waits on `scripts/mister status --json`,
launcher status fields, Main status fields, `/tmp/mister-magik/events.jsonl`,
and trace TSV row growth before falling back to timeout failure. The default
gate includes the expanded route, preview, velocity-scroll, controller, audio,
catalog mutation, first-boot scan, launch matrix, exit-menu, crash-loop,
display-mode, and install/restore checks. The 30-60 minute soak is deliberately
default-off behind `--soak`.

The default launch smoke target is:

```text
/media/fat/_Arcade/Missile Command (rev 3).mra
```

Override it with:

```bash
MISTER_ACCEPTANCE_LAUNCH_REF="/media/fat/path/to/game.mra" scripts/device-release-acceptance.sh --skip-deploy
```

Destructive first-boot catalog checks are opt-in:

```bash
scripts/device-release-acceptance.sh --skip-deploy --allow-reset-catalog
```

When enabled, the script backs up `/media/fat/mister-magik/library.sqlite3`
before removing it and records the backup path in the report.

The gate verifies supervised reboot while the launcher is active. It uses raw
reboot only as recovery after the exit-to-menu and game-handoff smokes, because
those tests intentionally leave launcher command mode.

The stock MiSTer OSD must not appear while the launcher owns the display. OSD is
acceptable during the explicit exit-to-menu and game-handoff portions of the
device gate, where the test intentionally leaves launcher ownership before
recovering with a reboot.

## Release Blockers

Block a public beta release if any of these fail:

- `scripts/mister doctor --json` reports an error.
- The launcher is not on `tty2`.
- The framebuffer is not in RGB565 launcher mode.
- More or fewer than one supervised launcher process is running.
- Main reports any invariant violation.
- The catalog database is missing, empty, or lacks `launcher_catalog`.
- Installed preview packs do not project nonzero image counts.
- Supervised Arcade restart does not reach the Arcade screen.
- Game launch handoff fails for `MISTER_ACCEPTANCE_LAUNCH_REF`.
- Crash-policy smoke does not report `LauncherCrashed`.
- Restart after crash-policy smoke does not return to `LauncherActive` without
  raw reboot recovery.
- Supervised reboot does not return to `LauncherActive`.
- The acceptance script does not produce a report and artifacts.

## Current Evidence

As of 2026-06-18, the host gate passes, and the fast hardware acceptance run
recorded under `build/device-release/20260618T073924Z/` passes. That run
records `LauncherCrashed`, returns to `LauncherActive` after crash-policy smoke
without raw reboot recovery, and reports an overall `PASS` result.

## Public Beta Limits

Known limits are acceptable for public beta if they are disclosed in release
notes:

- Live display geometry is still centered on the known stable 1080p HDMI path.
- Return-to-launcher after game reset may require reboot or manual recovery.
- Controller mapping and hot-plug behavior are still being polished.
- Real hardware acceptance is required before release because framebuffer route,
  Main handoff, input ownership, and exFAT catalog behavior cannot be proven in
  host CI.
