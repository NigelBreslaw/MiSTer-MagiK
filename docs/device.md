# Device operations and recovery

Use `scripts/magik device status`, `diagnostics` and `logs` for bounded evidence.
Launcher, display, media and catalog commands are documented in
[native operations](../magik/docs/native-operations.md). Device selection and
Keychain credentials are shared across worktrees; an IP override is optional.

## Boot-loop safety

Keep recovery attended and bounded. Never put reset faults into persistent
launcher configuration or automatically repeat an ambiguous reboot request.
`scripts/magik device recover --attended` clears owned volatile arming state and
resumes Main without rebooting. Inspect `device diagnostics` afterwards.

If the device repeatedly reboots, stop deployment attempts. Power it down and
mount the SD card on the Mac. Inspect the active installation's `launcher.env`
and `rebuild-on-next-boot`, remove fault-injection overrides, and inspect its
`bootlogs/main-reboot.log`. Preserve the normal Main-managed launch hook.

## Media diagnostics

The live report is `/tmp/mister-magik/media-diagnostics.json`. Persistent
reports live under the active installation's `diagnostics/media/` directory
and use schema `mister-magik-media-diagnostics-v1`. There are at most five
historical reports plus `latest.json`, each at most 64 KiB. Changed live state
is published at most once every two seconds; persistent failure snapshots are
limited to one per minute and sixteen attempts per boot, including failed
writes. A tmpfs ledger preserves that budget across launcher restarts. Repeated
failures and dropped queue events are counted. Healthy operation writes no
persistent media reports, and producers never wait for report IO.

Reports include bounded media events, build/session identity, pack/manifest
identity, requested/resolved preview paths, decoding stages, index fallback,
failed-cache suppression and presentation receipts. A decoded or applied image
does **not** prove it appeared on the physical display. If it remains black,
include the game/system, approximate time, output route, and a display photo
or a capture through the supported device capture command below. Do not use
raw framebuffer reads. No ROM/image contents, credentials, URL query strings,
automatic screenshots or uploads are included in media reports. Uninstalling
removes the installation's persistent reports.

Arcade is always reachable, even with zero installed games. Its status panel
distinguishes loading, an empty library and an unavailable library; Back/Home
remain usable. Recovery is through Library Refresh in Settings. Entering an
empty Arcade never creates or rebuilds a catalog, and a late shard result must
not reopen a screen the user has left.
