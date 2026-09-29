# Starting MiSTer MagiK

`Scripts/Start_MagiK.sh` is the only MagiK entry in the MiSTer Scripts menu. It
starts the production layout (`/media/fat/MiSTer_MagiK` and
`/media/fat/mister-magik/`) for the current boot without rebooting and without
changing `MiSTer.ini`. There is no install, restore, or uninstall step: a
reboot always returns to the Main selected by `MiSTer.ini`.

## Verification before start

The script verifies the fixed path and SHA-256 of `mister-magik-manager` from
the public `platform-v3.manifest`, then replaces itself with that Rust process
running `start public`. A missing hashing tool, manifest, or manager, or a
mismatched manager, fails before anything starts.

The manager verifies the complete public platform before anything is stopped:
every manifest component hash, the scanout and latch metadata bindings, and that
the scanout module was built for the running kernel. It applies the shared
`mister-magik-scanout-contract` profile rule, so a platform the frontend would
refuse is refused up front; 6.18 platforms are Development-only and never start
from the public layout. It also requires the hash-bound Main to carry the
session guard (`MISTER_MAGIK_SESSION_MAIN`). A platform whose Main predates it
fails `verify-platform`, and therefore the release delivery smoke, instead of
losing MagiK on the first game launch.

## Confirmation input

The start requires an explicit Down event from the keyboard or joystick.
Escape sequences may arrive in separate terminal reads; all remaining bytes must
arrive within a single 100 ms deadline after Escape. Interrupted reads do not
extend that deadline. Incomplete and unsupported sequences cancel, terminal
settings are restored, and nothing is started. Rejected escape sequences print a
bounded hexadecimal diagnostic, never arbitrary typed text.

## Session handoff

After confirmation a detached helper, logging to `/tmp/mister-magik-start.log`,
stops stock Main and starts `MiSTer_MagiK` with `MISTER_MAGIK_SESSION_MAIN`
naming that executable. The Main fork keeps that session across game launches
and launcher returns. If MagiK Main is not running after a bounded wait, the
helper restarts stock Main whenever any step after stopping it fails, and
reports both errors. Recovery never starts a second Main. An exclusive lock
(`/tmp/mister-magik-start.lock`, released by the kernel if the helper dies)
admits one handoff at a time; the helper re-checks the running Mains under the
lock. A running MagiK Main refuses the start. See
[the boot and process model](architecture.md#no-reboot-session-start).

Development boards select `MiSTer_MagiKDev` through `[MiSTer] main=` instead
(`scripts/magik device mode set --attended dev`), so Dev starts on every boot.

## Upgrades and leftover files

Earlier releases shipped `Scripts/MiSTer-MagiK.sh`, an installer that selected
MagiK in `MiSTer.ini`, and `Scripts/MiSTer-MagiK.platform-v3.constants.sh`.
Downloader removes both after they leave the database, except for users with
`allow_delete=0` or `allow_delete=2`. Those files, a `MiSTer.ini` that still
selects `MiSTer_MagiK`, and its `MiSTer.ini.bak.before-magik` backup are no
longer managed. To stop booting MagiK, set `[MiSTer] main=MiSTer`. Removing
MagiK files is manual.

If the script reports a missing or corrupt manager, re-run Downloader so the
manager and `platform-v3.manifest` come from the same release. Do not edit the
manifest.
