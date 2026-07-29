#!/bin/sh
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Stable MiSTer Scripts / Downloader entrypoint. Lifecycle policy lives in Rust.
set -eu

FAT="${MISTER_MAGIK_FAT:-/media/fat}"
MANIFEST="$FAT/mister-magik/platform-v3.manifest"
MANAGER="$FAT/mister-magik/mister-magik-manager"

fail() {
  echo "MiSTer MagiK: ERROR: $*" >&2
  exit 1
}

[ -r "$MANIFEST" ] || fail "missing $MANIFEST"
[ -f "$MANAGER" ] || fail "missing $MANAGER"
for tool in grep sed sha256sum awk chmod env; do
  command -v "$tool" >/dev/null 2>&1 || fail "required tool is unavailable: $tool"
done

manifest_value() {
  key="$1"
  count="$(grep -c "^$key=" "$MANIFEST" 2>/dev/null || true)"
  [ "$count" = 1 ] || return 1
  sed -n "s/^$key=//p" "$MANIFEST"
}

manager_path="$(manifest_value manager_path)" || fail "manifest has no unique manager_path"
[ "$manager_path" = /media/fat/mister-magik/mister-magik-manager ] || \
  fail "manifest manager_path is not canonical"
expected="$(manifest_value manager_sha256)" || fail "manifest has no unique manager_sha256"
case "$expected" in
  *[!0-9a-f]*|'') fail "manifest manager_sha256 is invalid" ;;
esac
[ "${#expected}" = 64 ] || fail "manifest manager_sha256 is invalid"
actual="$(sha256sum "$MANAGER" | awk '{print $1}')" || fail "cannot hash manager"
[ "$actual" = "$expected" ] || fail "manager hash mismatch"
chmod +x "$MANAGER" || fail "cannot make manager executable"

exec env MISTER_MAGIK_FAT="$FAT" \
  MISTER_MAGIK_INITTAB="${MISTER_MAGIK_INITTAB:-/etc/inittab}" \
  MISTER_MAGIK_TEST_MODE="${MISTER_MAGIK_TEST_MODE:-0}" \
  MISTER_MAGIK_TEST_OUTPUT_MODE="${MISTER_MAGIK_TEST_OUTPUT_MODE:-}" \
  MISTER_MAGIK_TEST_KEYS="${MISTER_MAGIK_TEST_KEYS:-}" \
  "$MANAGER" "$@"
