#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAKE="$TMP/mister"
LOG="$TMP/calls.log"

cat >"$FAKE" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$MAGIK_MODE_TEST_LOG"
if [[ "${1:-}" == run ]]; then
  command="${2:-}"
  if [[ "$command" == *"manifest='/media/fat/mister-magik-dev/platform-v2.manifest'"* ]]; then
    [[ "${MAGIK_MODE_TEST_DEV_VALID:-0}" == 1 ]]
    exit
  fi
  if [[ "$command" == *"manifest='/media/fat/mister-magik/platform-v2.manifest'"* ]]; then
    [[ "${MAGIK_MODE_TEST_PUBLIC_VALID:-0}" == 1 ]]
    exit
  fi
  if [[ "$command" == *"MiSTer.ini.bak.before-magik"* ]]; then
    [[ "${MAGIK_MODE_TEST_PUBLIC_BACKUP:-0}" == 1 ]]
    exit
  fi
  if [[ "$command" == *"selected_main="* ]]; then
    printf 'selected_main=MiSTer\nrunning_main=MiSTer:10\n'
  fi
fi
FAKE
chmod +x "$FAKE"

run_mode() {
  MAGIK_MODE_TEST_LOG="$LOG" MISTER="$FAKE" "$ROOT/scripts/magik-mode.sh" "$@"
}

: >"$LOG"
MAGIK_MODE_TEST_PUBLIC_VALID=1 MAGIK_MODE_TEST_DEV_VALID=1 run_mode status >"$TMP/status"
grep -q '^public=valid$' "$TMP/status"
grep -q '^dev=valid$' "$TMP/status"
grep -q '^selected_main=MiSTer$' "$TMP/status"
grep -q '^running_main=MiSTer:10$' "$TMP/status"
grep -q 'running=none' "$ROOT/scripts/magik-mode.sh"
! grep -Eq 'ini-|inittab|reboot' "$LOG"

: >"$LOG"
MAGIK_MODE_TEST_DEV_VALID=1 run_mode dev
grep -qx 'inittab-ensure-stock' "$LOG"
grep -qx 'ini-repair-boot' "$LOG"
grep -qx 'ini-select-main MiSTer_MagiKDev' "$LOG"
grep -qx 'reboot-wait' "$LOG"
grep -q '/media/fat/mister-magik/launcher.env' "$LOG"
grep -q '/media/fat/mister-magik-dev/launcher.env' "$LOG"
grep -q '/tmp/mister-magik/fs-fault-session' "$LOG"
! grep -Eq 'direct-reset' "$LOG"

: >"$LOG"
if MAGIK_MODE_TEST_DEV_VALID=0 run_mode dev >/dev/null 2>&1; then
  echo "invalid development platform was accepted" >&2
  exit 1
fi
! grep -Eq 'ini-|inittab|reboot' "$LOG"

: >"$LOG"
if MAGIK_MODE_TEST_PUBLIC_VALID=1 MAGIK_MODE_TEST_PUBLIC_BACKUP=0 run_mode public >/dev/null 2>&1; then
  echo "public update-only installation was accepted" >&2
  exit 1
fi
! grep -Eq 'ini-|inittab|reboot' "$LOG"

: >"$LOG"
MAGIK_MODE_TEST_PUBLIC_VALID=1 MAGIK_MODE_TEST_PUBLIC_BACKUP=1 run_mode public
grep -qx 'ini-repair-boot' "$LOG"
grep -qx 'reboot-wait' "$LOG"

: >"$LOG"
run_mode stock
grep -qx 'ini-select-main MiSTer' "$LOG"
grep -qx 'reboot-wait' "$LOG"
! grep -Eq 'direct-reset' "$LOG"

echo "magik-mode tests: ok"
