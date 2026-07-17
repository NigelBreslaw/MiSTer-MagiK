#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Reproduce the missing-summary startup path using the existing warm SQLite
# catalog and navigation projection. No catalog scan, rebuild, or persistence
# is allowed; the catalog artifacts must remain byte-for-byte unchanged.
set -euo pipefail

echo "ERROR: the missing V2 summary regression is retired; V3 registry startup has no summary sidecar" >&2
exit 2

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
source "$ROOT/scripts/lib/magik-layout.sh"
magik_layout_select dev

LABEL="${1:-warm-catalog-summary-missing-preview-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT="$ROOT/build/warm-catalog-summary-missing-preview-regression/$LABEL"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_ENV="$MISTER_MAGIK_LAUNCHER_ENV"
REMOTE_ENV_BACKUP="/tmp/mister-magik/summary-missing-preview-launcher.env"
REMOTE_DB="$MISTER_MAGIK_LIBRARY_DB"
REMOTE_NAV="$MISTER_MAGIK_APP_DIR/library.nav.lz4b"
REMOTE_SUMMARY="$MISTER_MAGIK_APP_DIR/library.summary.json"
REMOTE_SUMMARY_BACKUP="/tmp/mister-magik/library.summary.json.$LABEL"
HAD_ENV=0
SUMMARY_HIDDEN=0

if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi

mkdir -p "$OUT"

remote() {
  "$MISTER" run "$1"
}

fail() {
  echo "FAIL: $*" >&2
  remote "tail -160 '$REMOTE_LOG' 2>/dev/null || true" >&2 || true
  exit 1
}

device_unavailable() {
  echo "FAIL: device unavailable while $*" >&2
  exit 1
}

wait_event() {
  local event="$1" timeout="${2:-30}" deadline result
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    result="$(remote "if grep -q '^startup_timing	$event	' '$REMOTE_LOG' 2>/dev/null; then echo found; else echo pending; fi")" ||
      device_unavailable "waiting for $event"
    if [[ "$result" == *found* ]]; then
      echo "ok: $event"
      return 0
    fi
    sleep 1
  done
  return 1
}

preview_nonblack_pixels() {
  python3 - "$1" <<'PY'
import struct
import sys
import zlib

path = sys.argv[1]
data = open(path, "rb").read()
if not data.startswith(b"\x89PNG\r\n\x1a\n"):
    raise SystemExit("not a PNG")
pos = 8
width = height = None
idat = bytearray()
while pos + 12 <= len(data):
    size = struct.unpack(">I", data[pos:pos + 4])[0]
    kind = data[pos + 4:pos + 8]
    payload = data[pos + 8:pos + 8 + size]
    pos += 12 + size
    if kind == b"IHDR":
        width, height, depth, color = struct.unpack(">IIBB", payload[:10])
        if depth != 8 or color != 6:
            raise SystemExit("unexpected PNG format")
    elif kind == b"IDAT":
        idat.extend(payload)
    elif kind == b"IEND":
        break
raw = zlib.decompress(bytes(idat))
stride = width * 4
x0, y0, x1, y1 = 560, 102, 880, 422
count = 0
for y in range(y0, min(y1, height)):
    row = raw[y * (stride + 1):(y + 1) * (stride + 1)]
    if not row or row[0] != 0:
        raise SystemExit("unexpected PNG row filter")
    pixels = row[1:]
    for x in range(x0, min(x1, width)):
        r, g, b = pixels[x * 4:x * 4 + 3]
        if r or g or b:
            count += 1
print(count)
PY
}

capture() {
  local name="$1"
  "$MISTER" agent framebuffer-capture "$OUT/$name.png" --json "$OUT/$name.json" >/dev/null
  preview_nonblack_pixels "$OUT/$name.png"
}

cleanup() {
  local rc=$? cleanup_rc=0
  set +e
  if [[ "$SUMMARY_HIDDEN" -eq 1 ]]; then
    remote "if test -e '$REMOTE_SUMMARY_BACKUP'; then mv '$REMOTE_SUMMARY_BACKUP' '$REMOTE_SUMMARY'; else test -s '$REMOTE_SUMMARY'; fi" ||
      cleanup_rc=1
  else
    remote "rm -f '$REMOTE_SUMMARY_BACKUP'" || cleanup_rc=1
  fi
  if [[ "$HAD_ENV" -eq 1 ]]; then
    remote "mv '$REMOTE_ENV_BACKUP' '$REMOTE_ENV'" || cleanup_rc=1
  else
    remote "rm -f '$REMOTE_ENV' '$REMOTE_ENV_BACKUP'" || cleanup_rc=1
  fi
  "$MISTER" launcher-restart --timeout 30 >/dev/null || cleanup_rc=1
  if [[ "$rc" -eq 0 && "$cleanup_rc" -ne 0 ]]; then
    rc=$cleanup_rc
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM

remote "test -s '$REMOTE_DB'" >/dev/null || fail "warm catalog database is missing"
remote "test -s '$REMOTE_NAV'" >/dev/null || fail "warm navigation projection is missing"
remote "test -s '$REMOTE_SUMMARY'" >/dev/null || fail "warm catalog summary is missing"
DB_SHA_BEFORE="$(remote "sha256sum '$REMOTE_DB' | awk '{print \$1}'" | tail -1)"
NAV_SHA_BEFORE="$(remote "sha256sum '$REMOTE_NAV' | awk '{print \$1}'" | tail -1)"
SUMMARY_SHA_BEFORE="$(remote "sha256sum '$REMOTE_SUMMARY' | awk '{print \$1}'" | tail -1)"
if remote "test -e '$REMOTE_ENV'"; then
  HAD_ENV=1
  remote "cp '$REMOTE_ENV' '$REMOTE_ENV_BACKUP'"
else
  remote "rm -f '$REMOTE_ENV_BACKUP'"
fi

remote "rm -f '$REMOTE_LOG'"
"$MISTER" launcher-restart \
  --env MISTER_CATALOG_REFRESH=off \
  --env MISTER_LAUNCHER_START_SCREEN=arcade \
  --env MISTER_LAUNCHER_START_SYSTEM=arcade \
  --env MISTER_ARCADE_SELECTED_INDEX=6 \
  --env MISTER_PREVIEW_TRACE=1 \
  --timeout 30
wait_event preview_selected_applied 15 ||
  fail "warm-catalog precondition did not show the selected Arcade preview"
BEFORE_NONBLACK="$(capture before)"
echo "warm-catalog preview nonblack pixels: $BEFORE_NONBLACK"
(( BEFORE_NONBLACK > 1000 )) ||
  fail "precondition failed: selected Arcade screenshot was not visible"

SUMMARY_HIDDEN=1
remote "rm -f '$REMOTE_SUMMARY_BACKUP' '$REMOTE_LOG'; mv '$REMOTE_SUMMARY' '$REMOTE_SUMMARY_BACKUP'"

if ! "$MISTER" launcher-restart \
  --env MISTER_CATALOG_REFRESH=off \
  --env MISTER_LAUNCHER_START_SCREEN=arcade \
  --env MISTER_LAUNCHER_START_SYSTEM=arcade \
  --env MISTER_ARCADE_SELECTED_INDEX=6 \
  --env MISTER_PREVIEW_TRACE=1 \
  --timeout 10; then
  # The launcher-restart readiness probe currently waits for a ready catalog.
  # A live launcher with its projection still hydrating is the state under test.
  remote "pidof mister-magik-fb >/dev/null" ||
    fail "launcher process died during missing-summary startup"
  echo "ok: launcher active while warm projection hydrates"
fi

wait_event first_frame 10 ||
  fail "missing-summary launcher did not draw its first frame"

wait_event catalog_projection_ready 30 ||
  fail "existing navigation projection did not hydrate"
wait_event preview_selected_applied 15 ||
  fail "selected Arcade preview did not become exact after hydration"
AFTER_NONBLACK="$(capture after)"
echo "after preview nonblack pixels: $AFTER_NONBLACK"

remote "mv '$REMOTE_SUMMARY_BACKUP' '$REMOTE_SUMMARY'"
SUMMARY_HIDDEN=0
DB_SHA_AFTER="$(remote "sha256sum '$REMOTE_DB' | awk '{print \$1}'" | tail -1)"
NAV_SHA_AFTER="$(remote "sha256sum '$REMOTE_NAV' | awk '{print \$1}'" | tail -1)"
SUMMARY_SHA_AFTER="$(remote "sha256sum '$REMOTE_SUMMARY' | awk '{print \$1}'" | tail -1)"
[[ "$DB_SHA_AFTER" == "$DB_SHA_BEFORE" ]] ||
  fail "catalog database changed during no-rebuild regression test"
[[ "$NAV_SHA_AFTER" == "$NAV_SHA_BEFORE" ]] ||
  fail "navigation projection changed during no-rebuild regression test"
[[ "$SUMMARY_SHA_AFTER" == "$SUMMARY_SHA_BEFORE" ]] ||
  fail "catalog summary changed during no-rebuild regression test"
remote "test ! -e '$MISTER_MAGIK_APP_DIR/rebuild-on-next-boot'" ||
  fail "regression test unexpectedly armed a catalog rebuild"

if [[ "$AFTER_NONBLACK" -le 1000 ]]; then
  fail "Arcade screenshot is missing after existing-catalog hydration (pixels=$AFTER_NONBLACK)"
fi

echo "PASS: Arcade screenshot appeared after existing-catalog hydration; DB, navigation, and summary SHAs remained unchanged"
