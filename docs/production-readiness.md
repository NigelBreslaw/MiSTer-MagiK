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
- The distribution zip contains the installer script, `MiSTer_MagiK`,
  `mister-magik-fb`, the qualified scanout module and metadata, the production
  latch RBF and metadata under `mister-magik/fpga/`, and
  `mister-magik/platform-v2.manifest` and
  `mister-magik/platform-bundle-v0.2.json`, plus the numbered database provenance
  in `mister-magik/game-databases-manifest.json`. It contains neither root
  `menu.rbf` nor a production `experiments/` directory.
- Main, FPGA, and kernel binaries originate from an immutable main-qualified
  platform bundle; legacy v0.1 bundles remain a migration fallback only;
  PR artifacts are never eligible for publication.
- MAME and HBMAME metadata originate from the highest numbered immutable
  `game-databases-vN` release, with archive, manifest, and `SHA256SUMS` verified
  together. Application publication never rebuilds them or accepts raw database
  paths. Synthetic bundles are test fixtures only.
- Rollback is verified with `scripts/restore-stock-boot.sh` before publishing a
  beta build from a new release flow.
- The exact workflow candidate passes the full device gate before a reviewer
  approves the protected `publish-beta` or `publish-release` environment. See
  [Releases and update_all installation](releases.md).

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

## Device Acceptance Gate

The industry term for checks that must run on real hardware is
**hardware-in-the-loop (HIL)**. In this repo, use **device acceptance gate** for
the MiSTer HIL release gate and keep the canonical command as
`scripts/device-release-acceptance.sh`.

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

Run only the health tier while iterating on basic device telemetry:

```bash
scripts/device-release-acceptance.sh --skip-deploy --tiers health
```

Run targeted tiers when validating one part of the release gate:

```bash
scripts/device-release-acceptance.sh --skip-deploy --tiers framebuffer-route,launcher-lifecycle,catalog,handoff
```

Run the long soak only when explicitly requested:

```bash
scripts/device-release-acceptance.sh --skip-deploy --soak
```

The script uses only `scripts/mister` for device access and writes artifacts to
`build/device-release/<timestamp>/`, including status JSON, doctor output,
snapshots, logs, optional frame/profile files, `report.md`, `results.tsv`, and
`summary.json`.
Health and catalog probe logs are captured as artifacts when those tiers run.
The old `/dev/MrAudio` `audio-tone` probe is no longer part of the production
binary; audio validation is covered by the video path.

`report.md` is the human-readable evidence file. It includes the selected
tiers, skip options, every recorded check, a compact result table, and the final
PASS/FAIL result. `summary.json` is the machine-readable result with selected
tiers, skipped tiers, pass/fail/skip counts, artifact directory, start
timestamp, options, and final result. Skipped checks are recorded as `SKIP` and
must not be counted as passes. An aborted run should still collect artifacts and
write both `report.md` and `summary.json` with a failing result.

The device gate is telemetry-first: it waits on `scripts/mister status --json`,
launcher status fields, Main status fields, `/tmp/mister-magik/events.jsonl`,
and, when `MISTER_ACCEPTANCE_BENCH_TOOLS=1` is set, trace TSV row growth before
falling back to timeout failure. The gate can
run named tiers with `--tiers`: `health`, `framebuffer-route`,
`launcher-lifecycle`, `catalog`, `handoff`, `display-modes`, `install-restore`,
and `soak`. The default gate includes every tier except `soak`; `--fast`
preserves the quick non-destructive preset. The 30-60 minute soak is
deliberately default-off behind `--soak` or `--tiers soak`.

Bench-tools-only checks are optional HIL sub-scenarios. When
`MISTER_ACCEPTANCE_BENCH_TOOLS=0`, preview trace and velocity trace checks must
report `SKIP`, not `PASS`. Use `MISTER_ACCEPTANCE_BENCH_TOOLS=1` only when the
deployed binary was built with the matching bench-tools support.

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
before removing it and records the backup path in the report. The catalog tier
also clears `/media/fat/mister-magik/launcher.env` and the display benchmark
boot request before this reset so benchmark-only launcher state cannot suppress
the first-boot worker.

The non-destructive catalog mutation check uses an isolated temporary library
root shaped like `_Arcade` and `MISTER_LIBRARY_ROOTS`; it expects the fixture
database to grow from one synthetic MRA to two and does not scan the production
library.

The gate verifies supervised reboot while the launcher is active. It uses raw
reboot only as recovery after the exit-to-menu and game-handoff smokes, because
those tests intentionally leave launcher command mode.

Every non-fast launcher lifecycle run includes a mandatory four-sample
supervised reboot soak; it does not depend on the optional long-soak tier:

```bash
scripts/mister agent boot-profile 4 --timeout 60 --fail-on-timeout
```

Every sample must recover the agent port, SSH command execution, and Main
`LauncherActive` status. Any missed recovery is a release blocker.

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
- Any sample in the four-reboot supervised Ethernet soak misses agent, SSH, or
  `LauncherActive` recovery.
- The acceptance script does not produce `report.md`, `summary.json`, and
  artifacts.

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
