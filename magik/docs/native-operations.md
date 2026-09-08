# Native device, catalog and platform operations

All commands share device identity discovery, Keychain SSH bootstrap authentication,
identity-keyed service tokens, capability updates and result directories. An IP
override is optional. A compatible installed service is reused across branches.

## Device and catalog

Use `scripts/magik device status`, `diagnostics`, or `logs` for bounded evidence;
`launcher status|restart|return-to-launcher` for Main-owned lifecycle; `display
status` or `display set MODE --attended` for one display transaction; `mode status`
or `mode set dev|public|stock --attended` for the next explicit boot. Mode selection
does not reboot. `reboot --attended` sends one request and observes one bounded
boot transition; it never repeats an ambiguous reboot. `recover --attended`
clears owned arming state and resumes Main without rebooting.

`device catalog` provides `inspect`, `cores`, `metadata-qualification`,
`neogeo-family-audit`, `query --database system:ID --sql SQL`,
`screenshots --system ID`, and `screenshot-qualification --system ID`.
Production catalog routines and formats remain authoritative. Queries are
read-only, bounded and confined to the selected catalog root. Screenshot export
is the catalog's TSV identity report, not framebuffer capture. Qualification
validates installed pack hashes and an actual indexed image decode. Audits are
explicit and are never delivery prerequisites.

`device media check|download --system ID` uses production media manifests and
integrity validation. `device catalog publish --release-dir DIR` publishes a
verified compact database release. Both default to `--layout dev`; public is
explicit. `device catalog purge --confirm` is Dev-only, suspends the launcher,
checks the production completion marker and restores Main; no automatic reboot.
Raw reports and failures are retained in the command's result directory.

## Independent platform entrypoint

### Update published releases in one command

Run `scripts/magik update --attended` to download, verify and deploy the latest numbered platform
and game-database releases from `NigelBreslaw/MiSTer-MagiK`. Published prereleases
are included; drafts are excluded. Use `scripts/magik update --download-only`
to queue releases without contacting the device. Plain `update` installs database-only
changes, but requests attendance before a required platform activation.
Both releases must verify before the shared desired pair changes. Downloads live
under `$MISTER_MAGIK2_STATE/updates` (the normal shared state directory when unset).
The output includes the database release directory usable with `catalog publish`.

Run `scripts/magik deploy --attended` to install that pair on the selected MiSTer
before starting real MagiK. A platform change requires running and selected Dev
mode and one bounded reboot. Plain `deploy` stops before publication when attendance
is needed. Database-only updates require no reboot or attendance. Installed hashes
and versions determine what is outstanding; newer installed releases are never
downgraded. Normal GUI edits do not reinstall the platform. Mini, `check`, and
`watch` do not consume the queue.

The desired pair remains available for other devices; each device has separate
completion evidence. Platform and database publication journals are retained in
separate subdirectories of the deploy result. If the platform succeeds and the
database step fails, the next deploy retries only outstanding work. An interrupted
platform transaction is finished automatically only when its saved host journal
matches and a new boot is confirmed. Otherwise deployment stops with the stage ID
for explicit inspection/restoration; it never repeats an ambiguous reboot.

Every real-MagiK deployment with queued releases checks native boot registration,
including already-current devices and recovered platform transactions. Missing
registration is repaired idempotently before further publication or a completion
receipt. Database-only/current-platform registration needs no attendance or reboot;
platform activation still requires attendance. Receipts include `service_boot`.
This verifies startup configuration, not a witnessed successful future boot.

Native `service-boot-state`/`service-boot-install`
operations verify/register the service through MiSTer's `linux/user-startup.sh`
hook. Existing commands are preserved, with the original saved as
`mister-magik2/user-startup.before-magik`. No boot-mode changes or reboot loops
are added. Reboot rediscovery is native-only and bound to the selected board.
Unavailable service recovery is separate from platform activation; never repeat
an ambiguous reboot to recover connectivity.

Host download and build preparation check disk headroom before proceeding.
The CLI uses its frozen lock; private Slint testing dependencies are installed
only for `check`, not update/deploy. Build errors retain container diagnostics.
CLI output is unbuffered; discovery, download, reconciliation, preparation and
installation phases are emitted as they happen.

### What verification proves

`publication-state` includes `platform.runtime_verification` under the
`publication-runtime-evidence-v1` capability. Installed artifact hashes and
the platform's existing `verified` flag describe files, not every running component.
Main's running `/proc/PID/exe` SHA-256 is reported with `matches_installed`.
The scanout module's `loaded` flag is separate from its unknown loaded build
identity; FPGA identity is likewise unknown. `identity_verified: null` means
unsupported evidence, not success. The Linux kernel image is not managed by this
platform update (the scanout `.ko` is a kernel module). The CLI explicitly reports
these limits; older responses without runtime evidence are reported as unavailable.
Hardware acceptance must still verify a service return after an attended boot.

Download/verification errors preserve the previous desired pair; installation
errors retain the newly verified desired pair for recovery. Corrupt cached releases fail
verification rather than silently falling back. A deployment uses the desired
pair captured when it starts, even if another update completes concurrently.

Use `scripts/magik-platform`:

- `platform --root DIR --layout dev|public --attended --activate-fpga`
- `local-main --root DIR --source-checkout DIR --attended`
- `fpga-install --root DIR --signoff-report FILE --attended --activate-fpga`
- `restore --stage ID --attended`

The input tree is relative to `/media/fat`, with the binding `platform-v3.manifest`
and required components at their manifest paths. Normal platform publication
checks artifact hashes, activation and bounded startup health. No aggregate
board certificate, six-hour stress run or automatic display matrix is required.
Local Main requires a clean committed source checkout matching the manifest.
Experimental FPGA installation retains explicit timing/CDC signoff validation.

Publication snapshots replaced files, persists restoration information before
renaming each file, publishes the manifest last and synchronizes storage.
Local Main and FPGA-only changes resume the Dev launcher before Main's supervised
reload, then verify the new generation and executable hash. Full platform delivery
requires the selected boot mode to match the running layout and performs one
explicit reboot so the kernel module is activated too. The native service checks
new boot identity, installed hashes and launcher health before discarding backups.

A lost connection is not permission to replay a mutation. The host saves the stage
ID before upload. Failed activation retains backups and diagnostics. An explicit
`restore` reapplies the previous artifacts; a separately requested reboot activates
them. If the network/device is unavailable, use the existing SD-card recovery path.
There is no background recovery loop or unattended reboot.

## Validation evidence

During development, the planned read-only status/catalog session passed, followed
by one launcher restart: Main reported `LauncherActive`, PID 9200, in 3458 ms.
Status/discovery required no `MISTER_IP`; catalog inventory completed in 3343 ms.
Result IDs: `20260907T103109Z-488cdf1bad47`, `20260907T103251Z-b36cf960c4a5`,
`20260907T103312Z-47fd5984cd40`. Later cleanup used local validation only.
No platform activation, reboot, purge or media publication acceptance was run.
Those destructive operations require a separately identified attended session.
