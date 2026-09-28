#!/bin/sh
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# MiSTer Scripts entrypoint: start MiSTer MagiK Dev for this boot without
# rebooting. MiSTer.ini is unchanged; the verified manager owns the handoff.
set -eu

FAT="${MISTER_MAGIK_FAT:-/media/fat}"
LAYOUT=dev
APP="$FAT/mister-magik-dev"
MANIFEST="$APP/platform-v3.manifest"
MANAGER="$APP/mister-magik-manager"

fail() {
  echo "MiSTer MagiK: ERROR: $*" >&2
  exit 1
}

[ -r "$MANIFEST" ] || fail "missing $MANIFEST"
[ -f "$MANAGER" ] || fail "missing $MANAGER"
[ "$(grep -c '^manager_sha256=' "$MANIFEST")" = 1 ] || \
  fail "manifest has no unique manager_sha256"
expected="$(sed -n 's/^manager_sha256=//p' "$MANIFEST")"
actual="$(sha256sum "$MANAGER" | awk '{print $1}')" || fail "cannot hash manager"
[ -n "$expected" ] && [ "$actual" = "$expected" ] || fail "manager hash mismatch"
chmod +x "$MANAGER" || fail "cannot make manager executable"

exec env MISTER_MAGIK_FAT="$FAT" \
  MISTER_MAGIK_TEST_MODE="${MISTER_MAGIK_TEST_MODE:-0}" \
  MISTER_MAGIK_TEST_KEYS="${MISTER_MAGIK_TEST_KEYS:-}" \
  "$MANAGER" start "$LAYOUT"
