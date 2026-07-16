#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Replays an existing cached catalog through the real Ready -> UseCatalog ->
# bridge publication path. It never rebuilds or persists catalog state.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
source "$ROOT/scripts/lib/magik-layout.sh"

LAYOUT=dev
LABEL="launcher-catalog-publication-$(date -u +%Y%m%dT%H%M%SZ)"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --layout) LAYOUT="${2:?--layout needs dev or public}"; shift 2 ;;
    --label) LABEL="${2:?--label needs a value}"; shift 2 ;;
    -h|--help)
      echo "usage: scripts/device-launcher-catalog-publication-regression.sh [--layout dev|public] [--label LABEL]"
      exit 0
      ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$LAYOUT" in dev|public) ;; *) echo "--layout must be dev or public" >&2; exit 2 ;; esac
[[ "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]] || { echo "invalid label" >&2; exit 2; }

magik_layout_select "$LAYOUT"
OUT="$ROOT/build/device-launcher-catalog-publication-regression/$LABEL"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_ENV="$MISTER_MAGIK_LAUNCHER_ENV"
REMOTE_ENV_BACKUP="/tmp/mister-magik/catalog-publication-launcher.env"
READY_GATE="/tmp/mister-magik/catalog-publication-ready-$LABEL"
RELEASE_GATE="/tmp/mister-magik/catalog-publication-release-$LABEL"
SESSION_GATE="/tmp/mister-magik/catalog-publication-session-$LABEL"
REMOTE_DB="$MISTER_MAGIK_LIBRARY_DB"
REMOTE_NAV="$MISTER_MAGIK_APP_DIR/library.nav.lz4b"
REMOTE_SUMMARY="$MISTER_MAGIK_APP_DIR/library.summary.json"
REMOTE_PACK="$MISTER_MAGIK_ASSET_DIR/arcade-screenshots-320x320.mmlz4b"
REMOTE_PACK_LEGACY="$MISTER_MAGIK_ASSET_DIR/arcade-screenshots.mmlz4b"
HAD_ENV=0
ENV_STATE_CAPTURED=0
START_SECONDS=$SECONDS

mkdir -p "$OUT"

remote() {
  "$MISTER" run "$1"
}

fail() {
  echo "FAIL: $*" >&2
  remote "tail -160 '$REMOTE_LOG' 2>/dev/null || true" >&2 || true
  exit 1
}

wait_event() {
  local event="$1" timeout="$2" deadline result
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    result="$(remote "if grep -q '^startup_timing	$event	' '$REMOTE_LOG' 2>/dev/null; then echo found; else echo pending; fi")" ||
      fail "device unavailable while waiting for $event"
    if [[ "$result" == *found* ]]; then
      echo "ok: $event"
      return 0
    fi
    sleep 1
  done
  return 1
}

sha_remote() {
  remote "sha256sum '$1' | awk '{print \$1}'" | tail -1
}

tile_pixels() {
  python3 - "$1" <<'PY'
import struct
import sys
import zlib
from collections import Counter

data = open(sys.argv[1], "rb").read()
if not data.startswith(b"\x89PNG\r\n\x1a\n"):
    raise SystemExit("not a PNG")
pos, chunks = 8, bytearray()
width = height = None
while pos + 12 <= len(data):
    size = struct.unpack(">I", data[pos:pos + 4])[0]
    kind = data[pos + 4:pos + 8]
    payload = data[pos + 8:pos + 8 + size]
    pos += 12 + size
    if kind == b"IHDR":
        width, height, depth, color = struct.unpack(">IIBB", payload[:10])
        if (depth, color) != (8, 6):
            raise SystemExit("unexpected PNG format")
    elif kind == b"IDAT":
        chunks.extend(payload)
    elif kind == b"IEND":
        break
raw = zlib.decompress(bytes(chunks))
stride = width * 4
region = []
for y in range(120, min(470, height)):
    row = raw[y * (stride + 1):(y + 1) * (stride + 1)]
    if not row or row[0] != 0:
        raise SystemExit("unexpected PNG row filter")
    pixels = row[1:]
    for x in range(40, min(920, width)):
        region.append(tuple(pixels[x * 4:x * 4 + 3]))
background = Counter(region).most_common(1)[0][0]
print(sum(
    1 for rgb in region
    if sum(abs(channel - base) for channel, base in zip(rgb, background)) > 12
))
PY
}

cleanup() {
  local rc=$? cleanup_rc=0
  set +e
  remote "touch '$RELEASE_GATE'; rm -f '$READY_GATE' '$RELEASE_GATE' '$SESSION_GATE'" ||
    cleanup_rc=1
  if [[ "$ENV_STATE_CAPTURED" -eq 1 ]]; then
    if [[ "$HAD_ENV" -eq 1 ]]; then
      remote "mv '$REMOTE_ENV_BACKUP' '$REMOTE_ENV'" || cleanup_rc=1
    else
      remote "rm -f '$REMOTE_ENV' '$REMOTE_ENV_BACKUP'" || cleanup_rc=1
    fi
  fi
  remote "test ! -e '$READY_GATE' && test ! -e '$RELEASE_GATE' && test ! -e '$SESSION_GATE' && \
    ! grep -q 'MISTER_MAGIK_TEST_CATALOG_PUBLICATION\\|MISTER_MAGIK_TEST_FIRST_FRAME_RELEASE' \
      /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env 2>/dev/null && \
    test ! -e /media/fat/mister-magik/rebuild-on-next-boot && \
    test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot && \
    test ! -e /tmp/mister-magik/fs-fault-launcher.env && \
    test ! -e /tmp/mister-magik/fs-fault-session && \
    test ! -e /tmp/mister-magik/fs-fault.json" || cleanup_rc=1
  "$MISTER" launcher-restart --timeout 30 >/dev/null || cleanup_rc=1
  if [[ "$rc" -eq 0 && "$cleanup_rc" -ne 0 ]]; then rc=$cleanup_rc; fi
  exit "$rc"
}

remote "test -s '$REMOTE_DB'" >/dev/null || fail "catalog database missing"
remote "test -s '$REMOTE_NAV'" >/dev/null || fail "navigation projection missing"
remote "test -s '$REMOTE_SUMMARY'" >/dev/null || fail "catalog summary missing"
if remote "test -s '$REMOTE_PACK'"; then
  REMOTE_ACTIVE_PACK="$REMOTE_PACK"
elif remote "test -s '$REMOTE_PACK_LEGACY'"; then
  REMOTE_ACTIVE_PACK="$REMOTE_PACK_LEGACY"
else
  fail "Arcade screenshot pack missing"
fi

DB_SHA="$(sha_remote "$REMOTE_DB")"
NAV_SHA="$(sha_remote "$REMOTE_NAV")"
SUMMARY_SHA="$(sha_remote "$REMOTE_SUMMARY")"
PACK_SHA="$(sha_remote "$REMOTE_ACTIVE_PACK")"
if remote "test -e '$REMOTE_ENV'"; then
  HAD_ENV=1
  remote "cp '$REMOTE_ENV' '$REMOTE_ENV_BACKUP'"
else
  remote "rm -f '$REMOTE_ENV_BACKUP'"
fi
ENV_STATE_CAPTURED=1
trap cleanup EXIT INT TERM
remote "rm -f '$READY_GATE' '$RELEASE_GATE' '$REMOTE_LOG'"
remote "touch '$SESSION_GATE'"

if ! "$MISTER" launcher-restart \
  --env MISTER_CATALOG_REFRESH=default \
  --env MISTER_LAUNCHER_START_SCREEN=home \
  --env MISTER_MAGIK_TEST_CATALOG_PUBLICATION_SESSION="$SESSION_GATE" \
  --env MISTER_MAGIK_TEST_CATALOG_PUBLICATION_GATE="$READY_GATE" \
  --env MISTER_MAGIK_TEST_FIRST_FRAME_RELEASE_GATE="$RELEASE_GATE" \
  --timeout 6; then
  remote "pidof mister-magik-fb >/dev/null" ||
    fail "launcher process died before catalog replay"
fi
wait_event catalog_publication_test_waiting 10 ||
  fail "catalog publication test did not arm"
remote "touch '$READY_GATE'"
wait_event catalog_publication_test_first_frame_held 10 ||
  fail "first launcher frame was not held"

FIRST_FRAME_LINE="$(remote "grep '^startup_timing	launcher_first_frame_presented	' '$REMOTE_LOG' | tail -1")"
read -r NAV_ITEMS BRIDGE_ITEMS < <(python3 -c '
import re, sys
line = sys.stdin.read()
def field(name):
    match = re.search(r"(?:^|\s)" + re.escape(name) + r"=(\d+)", line)
    if not match:
        raise SystemExit(1)
    return int(match.group(1))
print(field("nav_menu_items"), field("bridge_menu_items"))
' <<<"$FIRST_FRAME_LINE")
(( NAV_ITEMS > 0 )) || fail "first frame navigation model is empty"
[[ "$BRIDGE_ITEMS" -eq "$NAV_ITEMS" ]] ||
  fail "first frame bridge model differs from navigation ($BRIDGE_ITEMS != $NAV_ITEMS)"

"$MISTER" agent framebuffer-capture "$OUT/first-home.png" \
  --json "$OUT/first-home.json" >/dev/null
HOME_PIXELS="$(tile_pixels "$OUT/first-home.png")"
(( HOME_PIXELS > 5000 )) ||
  fail "first Home frame has no visible system tiles (pixels=$HOME_PIXELS)"

[[ "$(sha_remote "$REMOTE_DB")" == "$DB_SHA" ]] || fail "catalog database changed"
[[ "$(sha_remote "$REMOTE_NAV")" == "$NAV_SHA" ]] || fail "navigation projection changed"
[[ "$(sha_remote "$REMOTE_SUMMARY")" == "$SUMMARY_SHA" ]] || fail "catalog summary changed"
[[ "$(sha_remote "$REMOTE_ACTIVE_PACK")" == "$PACK_SHA" ]] || fail "screenshot pack changed"
remote "test ! -e '$MISTER_MAGIK_APP_DIR/rebuild-on-next-boot'" ||
  fail "catalog rebuild marker was armed"

TOTAL_SECONDS=$((SECONDS - START_SECONDS))
(( TOTAL_SECONDS <= 30 )) || fail "regression exceeded 30 seconds ($TOTAL_SECONDS)"
echo "PASS: atomic catalog publication layout=$LAYOUT nav=$NAV_ITEMS bridge=$BRIDGE_ITEMS pixels=$HOME_PIXELS seconds=$TOTAL_SECONDS artifacts=$OUT"
