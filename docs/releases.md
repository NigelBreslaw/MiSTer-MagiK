# Releases and update_all installation

MiSTer MagiK is distributed through GitHub Releases and a MiSTer Downloader
database consumed by `update_all`. Publication is manual. The current public
channel is Beta.

## Install the Beta channel

The first install is a one-time bootstrap:

1. Download `mister-magik-beta-installer.zip` from the latest MiSTer MagiK
   GitHub prerelease.
2. Extract it at the root of the primary MiSTer SD card. It installs only
   `downloader_mister_magik.ini`, which points at the Beta database.
3. Boot the MiSTer and run `update_all`. Downloader fetches the complete
   platform without changing `MiSTer.ini` or boot configuration.
4. Run `Scripts` -> `MiSTer-MagiK` once. The installer verifies the platform
   manifest, every required file hash, the shared platform contract, module
   metadata, RBF metadata, and executables before enabling the handoff.
   It canonicalizes `[Menu] video_mode=8` (1920x1080 at 60 Hz), the only
   currently supported and release-tested launcher output mode. Before making
   changes, it warns that other resolutions and CRT support are coming later
   and requires A/Enter confirmation; any other input cancels.
   Before its first boot-configuration edit, it preserves the existing INI as
   `/media/fat/MiSTer.ini.bak.before-magik`. That backup is never replaced by a
   later install or reinstall.
5. Reboot normally. In Settings -> Info, confirm the displayed version matches
   the GitHub prerelease version.

The bootstrap does not replace stock `/media/fat/MiSTer`, root `menu.rbf`, or
any `MiSTer.ini`. If the download is partial, corrupt, or mixed between builds,
the installer exits before changing boot state.

After installation, update by running `update_all` and rebooting normally.
Existing catalogs, media, settings, snapshots, and user content are outside the
Downloader database and are preserved.

Running `Scripts` -> `MiSTer-MagiK` again while the MagiK handoff is active
opens a controller-friendly menu for restoring stock boot or fully uninstalling
MiSTer MagiK. Use UP/DOWN to select, A/Enter to continue, and B/Escape to
cancel. Full uninstall has a second A/Enter confirmation. If the pre-MagiK
backup is unexpectedly missing during a reinstall, the installer warns and
leaves it missing rather than copying the already-modified live INI under a
misleading backup name.

## Beta feed

The permanent Downloader identity is `mister_magik`; its installed drop-in
selects the Beta feed:

```text
https://raw.githubusercontent.com/NigelBreslaw/MiSTer-MagiK/downloader/mister-magik-beta-db.json.zip
```

The `downloader` ref is an orphan, artifact-only branch. Its root commit has no
source-history parent. The current release package intentionally includes no
channel-selection script.

## Restore and uninstall

To restore stock boot without deleting MiSTer MagiK data, run this from a
MiSTer shell and reboot normally:

```sh
sh /media/fat/Scripts/MiSTer-MagiK.sh restore
```

This canonicalizes the active Main as `main=MiSTer`, restores the stock inittab
shape, and retains the application, catalogs, settings, snapshots, media,
Downloader entry, scripts, and pre-MagiK INI backup. To enable it again, run
`Scripts` -> `MiSTer-MagiK` and reboot. The release-tested
`[Menu] video_mode=8` setting remains in place when stock boot is restored.

Choose **Fully uninstall MiSTer MagiK** from the menu, or run the interactive
`uninstall` command, to restore and verify stock boot before deleting MagiK's
application directory, Main fork, scripts, Downloader entry, saved backup,
optional agent hook, and legacy legal files. Uninstall never reboots
automatically and refuses to run without interactive confirmation. After a
successful install, restore, or uninstall, the script offers a normal reboot;
A/Enter syncs storage and reboots, while any other input exits without
rebooting. Downloader itself is not configured to reboot for MagiK updates.

If an update is interrupted, run `update_all` again before rebooting. The
platform manifest and hashes prevent activation of an incomplete initial
installation; the Main fork rejects an invalid latch/platform contract and
keeps the stock menu path available. Do not manually mix files from different
release ZIPs.

## Manual publication checklist

The workflow is `.github/workflows/distribution.yml` and has only the
`workflow_dispatch` trigger.

Before dispatch:

1. Merge all release changes to `main` and run `scripts/release-check-host.sh`.
2. When inputs listed in `scripts/platform-component-inputs/fpga-v0.1.txt`
   changed, run the FPGA Vblank Latch workflow on `main`. When inputs listed in
   `scripts/platform-component-inputs/kernel-v0.1.txt` changed, wait for the
   main Kernel scanout workflow or dispatch it on `main`. Userspace scanout ABI,
   deploy, installer, and checked-documentation changes require the lightweight
   Scanout contract check, not new platform artifacts. Pull-request builds are
   validation-only and do not create promotable artifacts.
3. Dispatch **Promote MiSTer MagiK Platform Bundle** from `main`. It captures
   the exact current `NigelBreslaw/Main_MiSTer:mister-magik` head, reuses or
   builds its content-addressed Main artifact, and reuses the exact current FPGA
   and kernel artifacts. The default dispatch uploads a candidate only. Inspect
   it, then repeat with `publish=true` and approve `publish-platform` to create
   the next immutable numbered release. A no-change dispatch publishes nothing.
4. Configure protected GitHub environments named `publish-platform`,
   `publish-game-databases`, `publish-beta`, and `publish-release`, with required
   reviewers. Keep repository-wide GitHub Release immutability disabled while
   Beta uses its rolling release; the numbered platform, database, and
   production workflows enforce immutability by refusing existing tags.
5. Dispatch **Promote MiSTer MagiK Game Databases** from `main`. The first run
   publishes `game-databases-v1`; later no-change runs exit without publishing.
6. Confirm rollback with `scripts/restore-stock-boot.sh` on the test MiSTer.

In GitHub Actions, choose **Publish MiSTer MagiK**, click **Run workflow**, make
sure the branch is `main`, and select `beta`. This is the only release input.
The workflow consumes the highest numbered published `platform-v0.N` bundle
(with legacy `platform-v0.1-<bundle-id>` compatibility) and the highest
numbered published `game-databases-vN` bundle. A v0.2 platform bundle already
contains the exact qualified Main binary, so normal publication does not check
out, install a toolchain for, or build Main,
the kernel, or the RBF. The v0.1 Main build remains only as a migration fallback.
It does not rebuild support databases or require the
platform bundle to match the current application source identity. If either
support release is absent, run its manual promotion workflow before retrying
publication. The application version is computed from the dispatched commit:

```text
build        = git rev-list --count <dispatched-commit>
version      = 0.2.<build>
beta tag     = beta
release tag  = v0.2.<build>
```

The build job uploads `mister-magik-0.2.<build>-candidate` before the protected
publish job can start. Download that exact candidate, run
`sha256sum --check SHA256SUMS`, install its distribution ZIP on the release-test
SD card, and run the complete device gate:

```bash
scripts/device-release-acceptance.sh --skip-deploy
```

Check the candidate contains the expected ZIP, individual Downloader assets,
channel database and bootstrap ZIP, `release-assets.json`, and `SHA256SUMS`.
Confirm Settings -> Info, the package filename, `mister-magik/release-v1.txt`,
and all database asset URLs use the same `0.2.<build>`. Test fresh Beta install,
Beta-to-Beta update, interrupted-update repair, non-destructive stock restore
and re-enable, and full uninstall. Approve `publish-beta` only after those
checks pass.

The publish job rejects non-`main` dispatches and verifies all candidate
checksums again. Beta publication creates or updates the single rolling
prerelease titled `MiSTer MagiK 0.2.<build> Beta`, replaces its assets, moves
the `beta` tag, and then updates only the Beta feed. After the new feed is live,
it removes superseded `v0.2.<build>` prereleases and their tags. This
deliberately keeps the repository at one application-beta tag. Production
releases use immutable `v0.2.<build>` tags and are never overwritten.

## Platform bundle recovery

- A non-main FPGA, scanout, Main, or promotion dispatch is intentionally rejected.
- A missing published platform release requires a successful main promotion; do
  not use an Actions artifact from a PR or another branch.
- Published numbered platform assets are the permanent component store and do
  not expire automatically. Actions artifacts are retained for 90 days and are
  only a transport/cache layer.
- Component resolution searches numbered platform releases newest-first for the
  exact identity, then a non-expired successful Actions artifact. Main builds
  only after both miss. A missing new FPGA or kernel identity requires its
  component workflow; unchanged published components are never rebuilt merely
  because an Actions artifact expired.
- Different components may be recovered from different historical platform
  releases. Their original workflow run IDs and source revisions remain in the
  new candidate. v0.1 bundles can recover FPGA and kernel; durable Main recovery
  begins with the first published v0.2 bundle.
- A Main receipt with the wrong fork revision, authority, toolchain, binary
  hash, or unsuccessful origin run is rejected rather than reused.
- A mismatched platform contract is a failed qualification and must be fixed
  before promotion; a platform release is never patched in place.
- If a release is manually deleted or corrupt, recovery tries the next older
  verified exact release, then a live Actions artifact, and finally rebuilds
  only the component that no durable source can supply.

## Numbered platform releases

Run **Promote MiSTer MagiK Platform Bundle** manually from `main`. The workflow
captures the authoritative Main fork head, computes the current
Main/FPGA/kernel bundle ID, and compares it with the manifest in
the highest published `platform-v0.N` release. When the identity changed it
publishes `platform-v0.(N+1)`; otherwise it reports that the current release is
up to date and publishes nothing. Numeric selection makes `v0.10` newer than
`v0.9` regardless of publication timestamps.

The v0.2 durable format contains `MiSTer_MagiK`, the latch RBF, the scanout
module, all three component receipts/origins, and checksums. Its identity is the
hash of the Main, FPGA, and kernel component IDs. Existing v0.1 two-component
bundles remain verifiable and publishable during migration, but count as
missing Main when promotion plans the next release. This deliberately forces
one v0.2 promotion even when FPGA and kernel have not changed.

Each component is independently content addressed. Promotion never rebuilds a
kernel or RBF merely because Main changed. “Latest Main” means the exact
authoritative branch head captured once at workflow start, not whichever commit
is newest when a later job happens to run. Candidate manifests record all three
source revisions, component IDs, hashes, workflow run IDs, and toolchain-bound
Main identity. A missing or expired artifact is recovery input, never permission
to substitute a nearby revision.

## Numbered game-database releases

Run **Promote MiSTer MagiK Game Databases** manually from `main`. It compares the
latest MAME GitHub release and latest numeric HBMAME tag with the manifest in the
highest published `game-databases-vN` release before starting expensive work.
When neither changed, the workflow exits successfully as already up to date.
When one changed, only that database is rebuilt; the other is verified and
reused. The first publication is `game-databases-v1` with
`mister-magik-game-databases-v1.zip`, and each later upstream change increments
the whole release number. These support releases are immutable prereleases so
they do not replace the application release reported by GitHub as latest.

Application distribution downloads the numbered ZIP,
`game-databases-manifest.json`, and `SHA256SUMS` as one release directory. The
packager accepts only `--game-databases-release-dir`; it does not accept raw
MAME/HBMAME SQLite paths. All three release assets must agree before safe
extraction. Temporary synthetic database bundles exist solely inside tests and
must never be uploaded or supplied to production packaging.
