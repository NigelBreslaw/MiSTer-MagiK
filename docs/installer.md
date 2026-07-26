# MiSTer MagiK installer and boot-configuration safety

`Scripts/MiSTer-MagiK.sh` is only the stable MiSTer Scripts, Downloader, and
update_all entrypoint. It verifies the fixed path and SHA-256 of
`mister-magik-manager` from `platform-v2.manifest`, then replaces itself with
that Rust process. A missing, malformed, or mismatched manager fails before any
boot configuration is changed.

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
keyboard or joystick. A/Enter may confirm a normal reboot, but it cannot approve
those safety boundaries.

If the Scripts entrypoint reports a missing or corrupt manager, do not edit
`MiSTer.ini` or `inittab` by hand. Re-run Downloader or reinstall the complete
MiSTer MagiK package so `mister-magik-manager` and `platform-v2.manifest` come
from the same release, then run the entrypoint again. The bootstrap refuses to
run a partial or mismatched package and leaves boot configuration unchanged.
