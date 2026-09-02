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
