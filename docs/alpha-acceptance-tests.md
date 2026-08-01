# Automated Alpha Acceptance Test Inventory

This document describes the tests that gate promotion of an immutable MiSTer
MagiK alpha candidate to the rolling `alpha` release. The hardware journey uses
the real published candidate, the official MiSTer Downloader, a physical MiSTer,
the production Slint UI, real Main handoff, and a fixed USB Video capture input.

The workflow is `.github/workflows/alpha-acceptance.yml`. Its hardware command is:

```text
scripts/agent alpha accept --candidate <downloaded-candidate> --output <evidence>
```

The whole hardware job has a 15-minute timeout. UI automation sessions are
volatile and individually bounded to 120 seconds. Runs are serialized so two
jobs cannot operate the same MiSTer concurrently.

## 1. Alpha publication checks

Before hardware acceptance exists, the alpha publication workflow proves that:

- publication is running from `main`, with checked-out `HEAD` equal to the
  workflow commit;
- build number is the commit count and version is `0.2.<build>`;
- the newest numbered platform bundle and game-database release download and
  verify successfully;
- the real ARM device frontend builds and has only permitted shared-library
  dependencies;
- required Main, manager, scanout module and metadata, latch RBF and metadata,
  third-party notices, and exact source revisions are present;
- `platform-v3.manifest` binds every platform component hash and revision;
- the distribution ZIP and flattened assets package successfully;
- the immutable candidate tag is derived from the verified archive SHA-256;
- the alpha Downloader database points only to that immutable tag;
- every generated published asset passes `SHA256SUMS`;
- package, embedded release metadata, Downloader database, channel, version,
  build, and candidate tag all agree;
- alpha publication contains no first-install channel file;
- an existing immutable candidate is accepted only when its target commit,
  checksum manifests, and every checksummed asset are byte-identical.

Publication creates an immutable prerelease and a 30-day workflow artifact. It
does not move rolling `alpha` or its Downloader feed.

## 2. Candidate selection and isolation

Before touching the MiSTer, the workflow checks that:

- the publication workflow completed successfully;
- the release tag matches
  `alpha-candidate-v0.2.<build>-<12 lowercase hexadecimal characters>`;
- the tag resolves to one exact 40-character Git commit;
- a candidate already carrying `alpha-acceptance-marker.json` is skipped;
- an automatic `workflow_run` accepts only the candidate built from that
  workflow's source commit;
- the hardware runner checks out that exact candidate commit, not current
  `main`;
- release assets are downloaded into a fresh run-specific directory.

The workflow can also be run manually for a named immutable candidate. Its
hourly schedule picks up an unaccepted candidate after the hardware runner has
been offline.

## 3. Immutable candidate integrity

The acceptance command rejects the candidate unless all of these checks pass:

- `release-assets.json` has the supported schema;
- version and build number agree;
- every name and relative path is bounded and cannot escape the candidate
  directory;
- every `SHA256SUMS` entry matches the downloaded file;
- `SHA256SUMS` covers `release-assets.json` and the alpha Downloader database;
- the distribution ZIP hash matches `release-assets.json`;
- every file declared in the release receipt exists in the ZIP with the exact
  byte length and SHA-256;
- every separately published flattened asset matches the corresponding ZIP
  member;
- the ZIP contains no undeclared or missing files;
- `release-v1.txt`, the release receipt, and the platform manifest agree on
  version and build;
- the GUI binary hash and MagiK source revision agree with the platform
  manifest;
- the immutable tag is recomputed from the verified version and archive hash;
- exact hashes are extracted for Main, the Slint GUI, manager, scanout module
  and metadata, latch RBF and metadata, and the platform manifest.

## 4. Real installation and reboot

The device-side transaction then checks and exercises the real update route:

- discover the physical MiSTer and authenticate the typed device agent;
- update the device agent when its bounded protocol/capability identity is old;
- reject more than one Downloader configuration owning `[mister_magik]`;
- preserve the existing canonical Downloader configuration, or create a
  temporary canonical file when the section is absent;
- point that temporary section at the immutable candidate's published alpha
  database;
- invoke the official Downloader entrypoint with only the `mister_magik`
  database selected;
- enforce a 240-second Downloader timeout;
- restore the original Downloader configuration even when installation fails;
- verify the installed platform manifest and every installed component against
  the candidate hashes;
- perform one supervised reboot;
- wait for the device to go down and return within bounded deadlines;
- wait for the launcher to become ready;
- run public platform verification and public runtime health checks;
- confirm the running Slint build version and source revision match the
  candidate exactly;
- record the active Main generation used by the UI journey.

No rolling `alpha` tag or Downloader feed moves if any installation or reboot
check fails.

## 5. Real UI journey

Every injected action waits until the resulting UI state has been presented.
The journey uses production button handling rather than test-only callbacks.

### Home and catalog readiness

1. Press Home.
2. Assert the effective view is `home`.
3. Assert the catalog reports ready.
4. Capture an RGB565 checkpoint and a 1920x1080 USB Video image.

### Arcade navigation and velocity

1. Press A to enter Arcade.
2. Assert the effective view is `arcade`.
3. Assert at least one game is selectable and its stable game ID is nonempty.
4. Capture Arcade through RGB565 and USB Video.
5. Hold Down for 350 ms, release all input, and allow motion to settle.
6. If the list contains more than one game, assert the selected index moved.
7. Capture the settled velocity result through RGB565.

### Real game launch and return

1. Record the selected game identity.
2. Assert the launcher is on Arcade, input-ready, idle, and has no overlay.
3. Assert the selected item is the expected safe `.mra` below `/media/fat`.
4. Press the production A-button launch path.
5. Assert Main keeps the same PID and generation while completing
   `HandoffComplete`.
6. Assert the launcher process exits and its current Slint status disappears.
7. Assert a real non-Menu core identity is loaded.
8. Request the typed return-to-launcher action without rebooting.
9. Assert Main still has the original PID and generation.
10. Assert a new launcher PID is active.
11. Assert the restored Slint process has the candidate version and revision,
    `return_from_game` startup mode, and enabled input.
12. Assert the UI returns to Arcade with the same selected game and idle launch
    state.
13. Start a replacement volatile automation session.
14. Capture the returned Arcade UI through RGB565 and USB Video.

Failure after launch performs a bounded typed return attempt. If safe return
cannot be proven, the result is classified as recovery-required.

### Filter drawer and search

1. Press Left and assert the drawer opens.
2. Press B and assert the drawer level is `Filters`.
3. Press Down and A to activate search.
4. Assert search is active.
5. Press A on the on-screen keyboard.
6. Wait up to one second for the query to become `A` and assert search remains
   active.
7. Capture the search UI through RGB565.

### Return home and nested catalog memory

1. Press Home and assert the effective view is `home`.
2. Capture the restored Home UI through RGB565 and USB Video.
3. Search the root menu for a nested `menu:` item.
4. Fail if the real catalog contains no nested hierarchy.
5. Open the nested menu and assert its identity differs from `menu:root`.
6. Move within the nested menu when more than one item exists.
7. Record the selected item and capture the nested menu through RGB565.
8. Press B and assert return to `menu:root`.
9. Re-enter the nested menu and assert both the menu identity and remembered
   selected item are restored.
10. Return to the root menu.

### Settings and final navigation

1. Move to Settings and open it.
2. Assert the effective view is `settings`.
3. Capture Settings through RGB565.
4. Move once within Settings, press B, and assert return to Home.
5. Close the volatile automation session and fail if cleanup cannot be proven.

## 6. Visual evidence

The journey currently produces these authoritative RGB565 checkpoints:

- `home`
- `arcade`
- `arcade-velocity`
- `arcade-return`
- `arcade-search`
- `post-navigation`
- `nested-menu`
- `settings`

Each checkpoint proves that:

- the requested action sequence was presented;
- the semantic snapshot remained stable during capture;
- the active framebuffer/latch sequence is valid;
- a PNG, its byte length, SHA-256, and matching JSON metadata were written.

The physical HDMI path is independently sampled through the fixed USB Video
input at:

- `home`
- `arcade`
- `arcade-return`
- `home-restored`

Each USB image must be a validated 1920x1080 JPEG and is recorded with byte
length and SHA-256. RGB565 proves the authoritative rendered buffer; USB Video
proves a real physical video signal. Neither replaces the other.

## 7. Receipt and promotion gate

Successful hardware acceptance writes `alpha-acceptance.json` and uploads the
whole evidence directory as a private Actions artifact retained for 30 days.
The hosted promotion job re-downloads the immutable candidate and refuses to
promote unless:

- the receipt schema is supported and `accepted` is true;
- the complete candidate identity exactly matches freshly verified assets;
- the installed runtime identity matches the candidate;
- launch evidence proves a real non-Menu core and return;
- there are at least six valid, uniquely labelled RGB565 checkpoints;
- required RGB565 checkpoints `home`, `arcade`, `arcade-return`,
  `arcade-search`, `nested-menu`, and `settings` exist;
- each checkpoint PNG and JSON metadata file matches its recorded size and
  SHA-256;
- there are at least three physical USB captures;
- required USB captures `home`, `arcade`, and `arcade-return` exist;
- each USB capture exists, matches its recorded size and SHA-256, and is
  1920x1080;
- all evidence paths and filenames are safe relative paths.

Only after these checks pass are the already-tested candidate bytes copied to
the rolling `alpha` release and its Downloader feed. Promotion does not rebuild
the binaries.

## 8. Catalog compatibility snapshots

Normal Rust/catalog CI also protects a checked-in immutable predecessor corpus:

```text
crates/catalog/tests/fixtures/compat/alpha-ef79bbb2-shard3-nav1-rich2/
```

That corpus records the public alpha format from revision
`ef79bbb2a46fe35a7182db2161457129ae9fa7d2`, including provenance and hashes.
Current tests prove that:

- the exact predecessor descriptor is classified as `UpgradeRequired`;
- future formats are rejected as unsupported;
- mixed old/new descriptors are rejected as corrupt;
- only the known legacy stamp maps to the predecessor descriptor;
- navigation schema v1 is rejected by current-only reading;
- compatibility reading upgrades navigation v1 in memory and supplies the
  missing current fields safely;
- lazy reading selects compatibility only for a missing legacy descriptor or
  the exact known predecessor;
- binding inspection maps an accepted predecessor or old projection contract
  to `UpgradeRequired`;
- synthetic downgraded shard schemas rebuild every affected system into a new,
  readable generation;
- legacy database state is extracted into the current standalone state store
  and no longer depends on the old database;
- launcher lifecycle decisions cover continuing with a usable old catalog and
  selecting an atomic rebuild;
- publication tests cover manifest-last visibility and conservative rejection
  of binding/state mismatches.

This is the first saved catalog compatibility snapshot, not yet the desired
multi-version corpus. The following coverage is still required and must not be
mistaken for a current alpha gate:

- add a new immutable fixture for every released persisted catalog format;
- verify every fixture's recorded hashes before use;
- open the complete saved catalog state, manifest, shards, scanner cache, and
  navigation together;
- run the production upgrade/rebuild decision from each predecessor;
- reopen after upgrade and prove the latest catalog authority is selected;
- prove interrupted upgrades retain the previous complete generation;
- prove corrupt newest authority falls back to the previous valid generation;
- optionally run one representative saved-catalog upgrade on the physical
  MiSTer before alpha promotion when its runtime cost is acceptable.

The current predecessor directory is only partially exercised: its decoded
navigation is used, but its provenance hashes, manifest, binding, and source MRA
are not yet consumed together as a complete historical authority tree. It also
does not contain real historical compressed navigation and SQLite shard files.

The fast hardware journey intentionally consumes the already-installed real
catalog rather than rebuilding it on every alpha. Full saved-catalog upgrades
belong in fast host fixtures, with only a small representative device smoke
test if needed.
