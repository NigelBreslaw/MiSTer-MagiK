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
semantics across repeated sections. Setting an owned key leaves exactly one
active assignment. Later duplicates are retained as comments so user context
is never silently discarded. Restore changes only installer-owned keys and
does not replace the live file with the backup.

Installation, 31 kHz output, and full uninstall require an explicit Down event
from the keyboard or joystick. A/Enter selects ordinary menu entries and may
confirm a normal reboot, but it cannot approve those safety boundaries.
