#!/bin/sh
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# MiSTer Scripts entrypoint: start MiSTer MagiK for this boot without
# rebooting. MiSTer.ini is unchanged; the verified manager owns the handoff.
set -eu

FAT="${MISTER_MAGIK_FAT:-/media/fat}"
APP="$FAT/mister-magik"
MANIFEST="$APP/platform-v3.manifest"
MANAGER="$APP/mister-magik-manager"

fail() {
  echo "MiSTer MagiK: ERROR: $*" >&2
  exit 1
}

for tool in grep sed sha256sum awk chmod env; do
  command -v "$tool" >/dev/null 2>&1 || fail "required tool is unavailable: $tool"
done
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
  "$MANAGER" start public
