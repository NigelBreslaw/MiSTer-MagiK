# Releases and update_all installation

MiSTer MagiK is distributed through GitHub Releases and a MiSTer Downloader
database consumed by `update_all`. Publication is manual. Beta is the public
testing channel; Alpha is an operator-only bootstrap channel used on the
maintainer's personal MiSTer before promotion to Beta.

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

## Alpha verification channel

Alpha is a rolling GitHub prerelease and Downloader feed for testing a build on
the maintainer's personal MiSTer. Publication intentionally produces no Alpha
installer ZIP and no downloadable `downloader_mister_magik.ini`. The Alpha
drop-in is created and retained locally by the operator; it is not a supported
public installation route and its contents are not published in this guide.

Dispatch **Publish MiSTer MagiK** from `main` with `alpha`, inspect the candidate,
and approve the protected `publish-alpha` environment. After `update_all` and
device verification succeed on the personal MiSTer, dispatch the same `main`
commit with `beta`. Beta publication fails before building unless the dispatched
commit exactly matches the current rolling `alpha` tag. Beta then rebuilds that
revision through the normal distribution pipeline; it does not reuse Alpha
artifacts byte-for-byte.

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

1. Merge all release changes to `main` and run `scripts/release/check-host.sh`.
2. Dispatch **Build MiSTer MagiK Platform** from `main`. It captures the exact
   current `NigelBreslaw/Main_MiSTer:mister-magik` head and computes the current
   Main, FPGA, and kernel identities. Unchanged components are verified and
   reused from the latest numbered platform release; changed components alone
   are built. Pull-request workflows are validation-only and never create
   promotable artifacts.
3. Leave `publish=false` to upload a candidate for inspection. Run again with
   `publish=true`; the workflow verifies and reuses that exact candidate without
   rebuilding its components, then waits for `publish-platform` approval. A
   no-change dispatch performs no heavyweight builds and creates neither a
   candidate nor a release.
4. Configure protected GitHub environments named `publish-platform`,
   `publish-game-databases`, `publish-alpha`, `publish-beta`, and
   `publish-release`, with required reviewers. Keep repository-wide GitHub
   Release immutability disabled while Alpha and Beta use rolling releases; the
   numbered platform, database, and production workflows enforce immutability
   by refusing existing tags.
5. Dispatch **Promote MiSTer MagiK Game Databases** from `main`. The first run
   publishes `game-databases-v1`; later no-change runs exit without publishing.
6. Confirm rollback with `scripts/magik-mode.sh stock` on the test MiSTer.

In GitHub Actions, choose **Publish MiSTer MagiK**, click **Run workflow**, make
sure the branch is `main`, and select `alpha`, `beta`, or `release`. This is the
only release input. Publish Alpha first for personal-device verification, then
dispatch Beta from the same commit after Alpha passes.
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
alpha tag    = alpha
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
channel database, `release-assets.json`, and `SHA256SUMS`. Beta and Release
candidates must also contain their bootstrap ZIP; Alpha must not contain one.
Confirm Settings -> Info, the package filename, `mister-magik/release-v1.txt`,
and all database asset URLs use the same `0.2.<build>`. Test fresh Beta install,
Beta-to-Beta update, interrupted-update repair, non-destructive stock restore
and re-enable, and full uninstall. Approve `publish-beta` only after those
checks pass.

The publish job rejects non-`main` dispatches and verifies all candidate
checksums again. Alpha publication creates or updates a rolling prerelease,
replaces its assets, moves the `alpha` tag, and updates only the Alpha feed; its
candidate and release contain no Alpha bootstrap ZIP. Beta publication requires
that exact Alpha revision, then creates or updates the single rolling
prerelease titled `MiSTer MagiK 0.2.<build> Beta`, replaces its assets, moves
the `beta` tag, and then updates only the Beta feed. After the new feed is live,
it removes superseded `v0.2.<build>` prereleases and their tags. This
deliberately keeps the repository at one application-beta tag. Production
releases use immutable `v0.2.<build>` tags and are never overwritten.

## Platform component reuse

- Platform artifact production has one manual entrypoint and rejects non-main
  dispatches. Main, FPGA, and kernel are not separately dispatchable.
- The latest verified v0.2 platform release is the sole reuse baseline and must
  contain all three components. Missing or invalid content is release corruption
  and fails the run; the workflow does not search older releases or Actions
  artifacts and does not rebuild a supposedly unchanged component.
- Changed components are built in the current workflow run. Unchanged components
  retain their exact binary content and are marked `reused-from-latest-release`.
- Exact changed-component artifacts and complete unpublished candidates from
  earlier unified platform runs on `main` are reusable for 30 days. Every cache
  hit is downloaded and independently identity-, provenance-, and checksum-
  verified before it can suppress a build. This permits retries after partial
  failure and makes the candidate-to-publish run fast.
- Component artifacts carry an immutable origin receipt and a checksum manifest
  for their complete cached file set. Re-uploading an artifact during a retry
  preserves the run ID and source revision of the run that actually built it.
- Actions artifacts are disposable caches, not alternate platform baselines.
  Components unchanged from the latest release still come only from that
  release; expired or invalid cached artifacts are ignored and rebuilt.
- A Main receipt with the wrong fork revision, authority, toolchain, binary
  hash, or unsuccessful origin run is rejected rather than reused.
- A mismatched platform contract is a failed qualification and must be fixed
  before promotion; a platform release is never patched in place.
- If the latest release is manually deleted or corrupt, repair the release
  history before attempting another platform build.

## Numbered platform releases

Run **Build MiSTer MagiK Platform** manually from `main`. The workflow
captures the authoritative Main fork head, computes the current
Main/FPGA/kernel bundle ID, and compares it with the manifest in
the highest published `platform-v0.N` release. When the identity changed it
publishes `platform-v0.(N+1)`; otherwise it reports that the current release is
up to date and publishes nothing. Numeric selection makes `v0.10` newer than
`v0.9` regardless of publication timestamps.

The v0.2 durable format contains `MiSTer_MagiK`, the latch RBF, the scanout
module, all three component receipts/origins, and checksums. Its identity is the
hash of the Main, FPGA, and kernel component IDs. Existing v0.1 two-component
bundles remain verifiable for historical consumers, but cannot be used as the
unified workflow baseline.

Each component is independently content addressed. Promotion never rebuilds a
kernel or RBF merely because Main changed. “Latest Main” means the exact
authoritative branch head captured once at workflow start, not whichever commit
is newest when a later job happens to run. Candidate manifests record all three
source revisions, component IDs, hashes, workflow run IDs, reuse/build status,
and toolchain-bound Main identity.

The normal review flow is two dispatches. First use `publish=false`, download
and inspect `platform-bundle-v0.N-candidate`, then dispatch with `publish=true`.
The second run selects the newest exact candidate matching the desired bundle
identity and next release number, verifies it again, skips all component builds
and assembly, and proceeds to the protected publication approval. If no valid
candidate remains, the same run reuses any exact verified component artifacts
and builds only what is still missing.

## Numbered game-database releases

Run **Promote MiSTer MagiK Game Databases** manually from `main`. It compares the
latest MAME GitHub release and latest numeric HBMAME tag with the manifest in the
highest published `game-databases-vN` release before starting expensive work.
When neither changed, the workflow exits successfully as already up to date.
When one changed, only that database is rebuilt; the other is verified and
reused. The first publication is `game-databases-v1` with
`mister-magik-game-databases-v1.zip`, and each later upstream change increments
the whole release number. For an intentional database-builder change, dispatch
with `force_mame_rebuild=true`; this publishes the next numbered release even
when the upstream identities are unchanged, rebuilding MAME while verifying and
reusing the current HBMAME database. These support releases are immutable
prereleases so they do not replace the application release reported by GitHub
as latest.

Application distribution downloads the numbered ZIP,
`game-databases-manifest.json`, and `SHA256SUMS` as one release directory. The
packager accepts only `--game-databases-release-dir`; it does not accept raw
MAME/HBMAME SQLite paths. All three release assets must agree before safe
extraction. Temporary synthetic database bundles exist solely inside tests and
must never be uploaded or supplied to production packaging.
