# MiSTer MagiK installer and boot-configuration safety

Interactive confirmation accepts Down (`ESC [ B` or `ESC O B`) only. Escape
sequences may arrive in separate terminal reads; all remaining bytes must arrive
within a single 100 ms deadline after Escape. Interrupted reads do not extend
that deadline. Incomplete and unsupported sequences cancel, terminal settings
are restored, and confirmation still precedes installation writes. Rejected
escape sequences print a bounded hexadecimal diagnostic, never arbitrary typed
text. This addresses fragmented input, not every possible controller mapping.

`Scripts/MiSTer-MagiK.sh` is only the stable MiSTer Scripts, Downloader, and
update_all entrypoint. It verifies the fixed path and SHA-256 of
`mister-magik-manager` from `platform-v3.manifest`, then replaces itself with
that Rust process. A missing, malformed, or mismatched manager fails before any
boot configuration is changed.

The launcher is self-contained and generated from the shared platform schema.
It is the only MagiK-owned entry under `Scripts`. The internal
`MiSTer-MagiK.platform-v3.constants.sh` helper is no longer shipped or loaded.
Do not edit the generated launcher; its maintained template and the schema are
checked by `scripts/checks/generate-platform-v3-consumers.py --check`.

The Rust manager owns installation, stock restoration, and uninstall. It
verifies the complete platform before mutation, snapshots the current boot
files, preserves the first pre-MagiK `MiSTer.ini` backup, and replaces edited
files through same-directory pending files with flush and read-back checks.
Uninstall restores and verifies stock boot before it stops processes or removes
owned files.

The shared `mister-magik-ini` crate follows Main_MiSTer's last-active-value
semantics across repeated sections. Installation changes only `[MiSTer] main`,
leaving every video and output setting untouched. It leaves exactly one active
Main assignment; later duplicates are retained as comments so user context is
never silently discarded. Restore changes only that Main assignment and does
not replace the live file with the backup.

Installation and full uninstall require an explicit Down event from the
keyboard or joystick. Successful installation reboots automatically after all
validation and boot-file replacement complete. A/Enter may confirm a normal
reboot after restore or uninstall, but it cannot approve those safety
boundaries.

If the Scripts entrypoint reports a missing or corrupt manager, do not edit
`MiSTer.ini` or `inittab` by hand. Re-run Downloader or reinstall the complete
MiSTer MagiK package so `mister-magik-manager` and `platform-v3.manifest` come
from the same release, then run the entrypoint again. The bootstrap refuses to
run a partial or mismatched package and leaves boot configuration unchanged.

Full uninstall uses cached Downloader 2.4 to remove MagiK's registration and
configuration, so an unchanged `update_all` feed can reinstall it. Other
Downloader state is preserved. A failed removal leaves stock boot selected
and a recovery executable at `/tmp/mister-magik-manager-recovery`; resolve the
reported Downloader error and run that executable with `uninstall` before
rebooting. ZIP-only installations without Downloader state use local removal.

## Obsolete helper and upgrades

Normal `update_all` runs use Downloader's managed-file deletion to remove the
old helper after it disappears from our database. There is no MagiK startup
cleanup service. Users with `allow_delete=0` or `allow_delete=2` may retain the
helper; the new launcher ignores it. Full uninstall still removes the legacy
helper explicitly. Unrelated Scripts entries are not owned by MagiK.

Manually extracting a ZIP does not delete obsolete files. After installing and
verifying the complete new package, the obsolete
`/media/fat/Scripts/MiSTer-MagiK.platform-v3.constants.sh` file can be removed
manually. Do not remove it from an older package that still requires it.

## Support response for the incorrect beta manifest

After the corrected beta is published:

> The `manifest manager_path is not canonical` error was a release packaging
> bug: the public package contained development paths. The installer stopped
> before changing boot settings. Run `update_all` to completion, then run
> `Scripts` → `MiSTer-MagiK` again. Do not edit the manifest or boot files.

Before that corrected release is available, rerunning the same feed cannot
repair the bad manifest. See [the release gate and rollout](releases.md).
