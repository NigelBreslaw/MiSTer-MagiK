#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_DB="/media/fat/mister-magik/library.sqlite3"
REMOTE_ASSETS="/media/fat/mister-magik/assets"
SETTLE_SECS=5
RACE_REFRESH=0

usage() {
  cat <<'USAGE'
usage: scripts/device-catalog-acceptance.sh [--settle SECS] [--race-refresh]

Checks the deployed MiSTer catalog state through scripts/mister:
  - exactly one launcher process
  - no active library-refresh after settling
  - non-empty library.sqlite3
  - launcher_catalog exists
  - screenshot packs project nonzero has_preview counts where installed
  - screenshot packs remain runtime-only and are not indexed into asset tables
  - optional duplicate refresh race proves one refresh skips via single-flight
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --settle)
      SETTLE_SECS="${2:?--settle needs seconds}"
      shift 2
      ;;
    --race-refresh)
      RACE_REFRESH=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

remote() {
  "$MISTER" run "$1"
}

db() {
  "$MISTER" db "$1"
}

last_number() {
  awk 'NF { value=$NF } END { gsub(/[^0-9]/, "", value); print value }'
}

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$actual" != "$expected" ]; then
    echo "FAIL: $label expected=$expected actual=$actual" >&2
    exit 1
  fi
  echo "ok: $label = $actual"
}

assert_gt_zero() {
  local label="$1" actual="$2"
  if [ -z "$actual" ] || [ "$actual" -le 0 ]; then
    echo "FAIL: $label expected > 0 actual=${actual:-empty}" >&2
    exit 1
  fi
  echo "ok: $label = $actual"
}

pack_exists() {
  remote "test -f '$REMOTE_ASSETS/$1' && echo yes || echo no" | awk 'NF { value=$NF } END { print value }'
}

preview_count_for_platform() {
  local platform="$1"
  db "SELECT COALESCE(SUM(has_preview),0) FROM launcher_catalog WHERE system_id='$platform';" | last_number
}

arcade_pack_exists() {
  remote "test -f '$REMOTE_ASSETS/arcade-screenshots.mmlz4b' && echo yes || echo no" | awk 'NF { value=$NF } END { print value }'
}

echo "==> Waiting ${SETTLE_SECS}s for startup refreshes to settle"
sleep "$SETTLE_SECS"

launcher_count="$(
  remote "ps w | grep '[m]ister-magik-fb ui launcher' | wc -l" | last_number
)"
assert_eq "launcher process count" "1" "$launcher_count"

refresh_count="$(
  remote "ps w | grep '[m]ister-magik-fb library-refresh' | wc -l" | last_number
)"
assert_eq "active library-refresh count" "0" "$refresh_count"

remote "test -s '$REMOTE_DB'"
echo "ok: $REMOTE_DB is present and non-empty"

launcher_catalog_tables="$(
  db "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='launcher_catalog';" | last_number
)"
assert_eq "launcher_catalog table count" "1" "$launcher_catalog_tables"

if [ "$(arcade_pack_exists)" = "yes" ]; then
  assert_gt_zero "arcade has_preview count" "$(preview_count_for_platform arcade)"
fi
if [ "$(pack_exists "neogeo-screenshots.mmlz4b")" = "yes" ]; then
  assert_gt_zero "neogeo has_preview count" "$(preview_count_for_platform neogeo)"
fi
if [ "$(pack_exists "saturn-screenshots.mmlz4b")" = "yes" ]; then
  assert_gt_zero "saturn has_preview count" "$(preview_count_for_platform saturn)"
fi

console_pack_count="$(
  remote "ls '$REMOTE_ASSETS'/nes-screenshots.mmlz4b '$REMOTE_ASSETS'/snes-screenshots.mmlz4b '$REMOTE_ASSETS'/n64-screenshots.mmlz4b '$REMOTE_ASSETS'/sms-screenshots.mmlz4b '$REMOTE_ASSETS'/megadrive-screenshots.mmlz4b '$REMOTE_ASSETS'/saturn-screenshots.mmlz4b 2>/dev/null | wc -l" | last_number
)"
if [ "$console_pack_count" -gt 0 ]; then
  asset_entry_tables="$(
    db "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='asset_entries';" | last_number
  )"
  assert_eq "runtime-only screenshot asset table count" "0" "$asset_entry_tables"
fi

if [ "$RACE_REFRESH" -eq 1 ]; then
  echo "==> Triggering duplicate library-refresh race"
  race_output="$(
    remote "mkdir -p /tmp/mister-magik; rm -f /tmp/mister-magik/refresh-race-a.log /tmp/mister-magik/refresh-race-b.log; '$REMOTE_BIN' library-refresh >/tmp/mister-magik/refresh-race-a.log 2>&1 & first=\$!; sleep 0.3; '$REMOTE_BIN' library-refresh >/tmp/mister-magik/refresh-race-b.log 2>&1; second_status=\$?; echo second_status=\$second_status; cat /tmp/mister-magik/refresh-race-b.log; wait \$first"
  )"
  echo "$race_output"
  if ! printf '%s\n' "$race_output" | grep -q 'library_refresh[[:space:]]skipped[[:space:]]active_pid='; then
    echo "FAIL: duplicate refresh did not report single-flight skip" >&2
    exit 1
  fi
  echo "ok: duplicate refresh skipped via single-flight"
fi

echo "device catalog acceptance: ok"
