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

## Device and platform delivery

Platform component IDs hash selected input contents; the last-changing Git
revision remains provenance, not part of the cache key. The platform workflow
contributes each component's build job and shared build settings, rather than
unrelated planning and publication steps. The explicit FPGA build date is a
synthesis input. Host protocol/scanout integration checks run before reuse, so
retiring an agent adapter does not by itself rebuild the RBF or module.

Before planning a release, compare the latest published component's recorded
source tree with the current selected inputs. Identical inputs may reuse the
original component ID and receipts even across reconstructed Git history or an
identity-algorithm update. Missing source objects or changed inputs require a
new build; published receipts are never relabelled. The complete baseline
archive is still downloaded and verified before assembly. FPGA report-validation
changes remain component inputs, while the separate synthesis key allows the
existing RBFs to be checked again without rerunning Quartus.

Kernel cache attestation is intentionally usable with the Ubuntu 20.04 build
container's Python 3.8. The CLI loads FFmpeg and database dependencies only for
operations that use them; the general host-tooling Python requirement is unchanged.

Use `scripts/magik deploy` for development applications and
`scripts/magik-platform` for explicit platform/Main transactions. Normal platform
delivery checks artifact integrity, activation and bounded startup health.
Physical qualification is requested separately; there is no aggregate certificate
or mandatory stress matrix. Distribution publication does not authorize device work.
