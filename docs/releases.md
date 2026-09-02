# Releases

The **Publish MiSTer MagiK** workflow is the only distribution publication path.
Python `scripts/magik-ci` owns orchestration; the Rust manifest contract and
manager own platform validation. FPGA/platform qualification remains separate.

## Build once, validate, then promote

1. Commit and push the complete change. Wait for CI for that exact commit.
2. Dispatch `distribution.yml` from `main` with `release_channel=alpha` and no
   `candidate_version`. Only alpha builds a payload and resolves qualified
   platform/database inputs.
3. The required gate compares the actual distribution ZIP and Downloader
   payloads, validates their paths, hashes and identities, runs the shipped ARM
   installer under emulation, and tests real Downloader fresh/upgrade delivery.
4. Approve `publish-alpha`. The workflow publishes immutable `v0.2.<build>`
   assets, downloads and verifies them, then updates the alpha feed.
5. Promote with `release_channel=beta` and the exact `candidate_version`, such
   as `0.2.6300`. Dispatch from the same source commit on `main`. Promotion
   downloads that version, revalidates the same bytes, and requires the current
   alpha feed to identify that candidate. Approve `publish-beta`.
6. Stable promotion uses `release_channel=release` and the same version. It
   requires the current beta candidate and updates the beta and release feeds
   together in one non-forced Git commit. Approve `publish-release`.

Payloads are never rebuilt during promotion. The versioned release's payload,
validation receipt and delivery evidence are immutable. Only channel-specific
feed/bootstrap files and release presentation metadata change. A conflicting
existing asset fails publication; identical or interrupted uploads can be
reconciled on retry. Upload/verification failures do not advance channel feeds.

The `alpha` and `beta` release entry URLs remain available for channel metadata
and installer drop-ins. They are not the source of new payload binaries. New
feeds reference versioned asset URLs. Legacy rolling assets and old versioned
releases are retained so older feeds continue to resolve their original files.
Do not delete them or infer candidate identity from rolling Git tags.

## One authoritative distribution gate

For a flat candidate directory, with native validators already built:

```bash
scripts/magik-ci ci distribution verify CANDIDATE --channel alpha --write-receipt
scripts/magik-ci ci distribution verify CANDIDATE --channel alpha
```

The first invocation creates `validated-candidate.json`; later verification
compares it rather than replacing it. It binds the exact archive, installed
assets, repository/source revision, platform identity and database identity.
CI's `distribution test-delivery` adds `delivery-evidence.json` for that exact
candidate. Publication requires both and revalidates the candidate after
downloading the CI artifact. `scripts/release/check-host.sh` delegates to the
same package gate; it is not an alternative qualification authority.

Manifest generation and verification require explicit `--layout public|dev`.
Build the read-only host checker with `scripts/cargo build --locked
--manifest-path mister/platform/contracts/manifest/Cargo.toml --bin
platform-manifest-check`, and the native manager with `scripts/cargo build
--locked --manifest-path mister/tools/manager/Cargo.toml`. Missing validators
fail closed. There is no weaker Python fallback or automatic Cargo bootstrap.

The pre-push hook remains the bootstrap-free Python fast gate. CI owns the
Rust/ARM and delivery matrix. Emulation verifies the installer; it does not
claim physical HDMI, FPGA or device qualification. Downloader testing uses a
pinned upstream revision and a loopback proxy; only transport URLs are remapped,
never installed bytes, manifest fields, sizes or hashes.

The gate also runs the real `update_all.pyz` entrypoint against the same local
feed. It qualifies the baseline Downloader revision, the device-compatible
revision used by cached fallback, and Update All as one pinned dependency set.
The Linux job creates an isolated `/media/fat` mount and runs the shipped ARM
manager under QEMU; macOS checks do not claim this matrix has run.

## Downloader lifecycle guarantee

An unchanged feed has two different meanings depending on the lifecycle stage:

* A second run with the registration and files intact is a cached fast path. It
  must make no payload requests and must not change boot files.
* `restore` returns stock boot selection while retaining the MagiK package and
  its Downloader registration. Running the same feed after that restore remains
  a cached path.
* Full `uninstall` first unregisters `mister_magik` through Downloader, then
  removes the package, generated data, and known legacy files. It retains other
  databases, sections, files, and Downloader caches. The next unchanged run is
  therefore a real download of the identical release, with every receipt hash
  checked again.

A changed feed is not repaired by forcing integrity mode: its new database
revision downloads normally under the selected checking mode. The required
matrix covers `balanced`, `fastest`, `exhaustive`, and named
`verify_integrity`, with each of `allow_delete=0`, `1`, and `2`, through both
direct Downloader and real Update All entrypoints.

If Downloader state is unreadable, externally unverified, unsupported, or the
updater is unavailable, the manager stops before package removal and reports
that Downloader must be updated. A nonzero, timeout, or state/configuration
failure during delegation leaves a verified recovery manager staged and offers
no reboot. Restore connectivity/state and retry the normal entrypoint; do not
delete fingerprint files, edit Downloader JSON, clear caches, or use
`--force`. A ZIP-only package with no Downloader registration follows the local
uninstall path.

For users who received the historical broken package, restore the correct
channel's Downloader INI/database entry first. As a temporary recovery setting
in the main Downloader configuration (normally `/media/fat/downloader.ini`),
use the named value `file_checking=verify_integrity`, not numeric `3`, then let
the corrected beta complete. Restore the normal checking preference afterward.
The corrected database changes the feed identity and downloads normally. The
new alpha is verified against its actual public payload before approval, and
beta promotion reuses those exact bytes; the immutable `0.2.6229` artifacts are
never modified.

To exercise this gate from a feature branch without publishing, dispatch the
workflow with `qualification_only=true` and `release_channel=alpha`. It builds
and uploads only a short-lived candidate artifact; publication remains limited
to an explicit main-branch dispatch and approval.

## Corrected-release checklist

- [ ] The broken Dev-as-public manifest is rejected by the mandatory gate.
- [ ] Native lifecycle, ZIP/feed parity and shipped ARM verification pass.
- [ ] Real Downloader fresh/upgrade tests pass, including deletion preferences.
- [ ] Publish alpha through the workflow and verify its downloaded artifacts.
- [ ] Promote those identical bytes to beta through the approval gate.
- [ ] Confirm the public beta feed references that immutable version, then tell
      affected users to rerun `update_all` and run the MagiK installer.

Immutable hosting prevents server-side mixed releases. It does not make an
interrupted SD-card update atomic. Users should let Downloader complete before
rebooting or running the installer; incomplete packages fail verification.

## Attended device/platform qualification

The pre-push hook and CI must pass for the exact release commit before delivery
and explicitly requested attended qualification (`scripts/agent deliver`, then
`scripts/agent release qualify`). Distribution publication does not implicitly
authorize or schedule these physical-device operations.
Development delivery builds the app from its clean local commit and never
pushes automatically. Main, the scanout kernel module, and the latch RBF come
from the latest qualified GitHub platform release, with its verified
tag-addressed archive reused across deliveries.

The platform workflow preserves completed stock and patched Quartus synthesis
under a synthesis-only content identity before qualification begins. If a
validator fails and its logic is subsequently fixed, the next run may restore
those identical RBF outputs, but it always reruns the complete FPGA validation
and creates fresh component provenance before assembly or publication.

The attended qualification is fixed and flag-free. It checks runtime, catalog,
input/handoff/return, display, recovery capability, and restoration. Rollback is
the typed `scripts/agent device mode set stock --attended` operator command. Packaging output, credentials,
caches, and private fixtures must not be staged.

CRT qualification additionally records a
`mister-magik-crt-qualification-v3` evidence document. It binds the exact app,
Main, and Menu revisions; kernel, RBF, platform-contract, platform-manifest,
protocol, and FPGA-component hashes; the local alpha device journey; bounded
RGB565 publication trial; resolved standard mode; and external analyzer
measurements for clock, totals, porches, sync widths, polarity, and rates. It
also records launcher/OSD/input, native-core-timing handoff, game lifecycle,
HDMI regression, cleanup, limitations, and rollback. Until that attended
real-CRT document passes
`scripts/checks/verify-crt-qualification-evidence.py`, CRT remains implemented
but not hardware-qualified.

Retained protocol-v2 RBF or CRT evidence is rollback-only and must be verified
with the explicit `--historical-v2` option. It cannot qualify a new platform.
