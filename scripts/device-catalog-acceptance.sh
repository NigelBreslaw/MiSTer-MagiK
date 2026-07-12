#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_DB="/media/fat/mister-magik/library.sqlite3"
REMOTE_SUMMARY="/media/fat/mister-magik/library.summary.json"
REMOTE_NAVIGATION="/media/fat/mister-magik/library.nav.lz4b"
REMOTE_ASSETS="/media/fat/mister-magik/assets"
EXPECTED_DURABLE_GAMES=67603
EXPECTED_VISIBLE_GAMES=65267
EXPECTED_SYSTEMS=72
EXPECTED_PC88_GAMES=3831
SETTLE_SECS=5
RACE_REFRESH=0

usage() {
  cat <<'USAGE'
usage: scripts/device-catalog-acceptance.sh [--settle SECS] [--race-refresh]

Checks the deployed MiSTer catalog state through scripts/mister:
  - exactly one launcher process
  - no active library-refresh after settling
  - non-empty library.sqlite3
  - current generic catalog facts contain games and discoveries
  - current summary and navigation projections are present
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

first_result_number() {
  awk '
    /^library_sql_timing_tsv[[:space:]]/ { next }
    NF && seen_header {
      value=$1
      gsub(/[^0-9]/, "", value)
      print value
      exit
    }
    NF { seen_header=1 }
  '
}

db_scalar() {
  db "$1" | first_result_number
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

pack_exists_for_platform() {
  local platform="$1"
  remote "if test -f '$REMOTE_ASSETS/${platform}-screenshots-320x320.mmlz4b' || test -f '$REMOTE_ASSETS/${platform}-screenshots.mmlz4b'; then echo yes; else echo no; fi" | awk 'NF { value=$NF } END { print value }'
}

arcade_pack_exists() {
  pack_exists_for_platform arcade
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

assert_eq "durable game row count" "$EXPECTED_DURABLE_GAMES" "$(db_scalar "SELECT count(*) FROM game_rows;")"
assert_eq "PC-8801 game row count" "$EXPECTED_PC88_GAMES" "$(db_scalar "SELECT count(*) FROM game_rows g JOIN string_values s ON s.string_id=g.system_string_id WHERE s.value='pc88';")"
assert_eq "PC-8801 boot ROM game row count" "0" "$(db_scalar "SELECT count(*) FROM game_rows g JOIN string_values s ON s.string_id=g.system_string_id WHERE s.value='pc88' AND lower(g.title)='boot';")"
assert_gt_zero "durable discovery count" "$(db_scalar "SELECT CAST(value AS INTEGER) FROM meta WHERE key='discoveries';")"
remote "test -s '$REMOTE_SUMMARY'"
echo "ok: $REMOTE_SUMMARY is present and non-empty"
summary_copy="$(mktemp)"
trap 'rm -f "$summary_copy"' EXIT
"$MISTER" get "$REMOTE_SUMMARY" "$summary_copy" >/dev/null
read -r visible_games visible_systems < <(
  python3 -c 'import json,sys; data=json.load(open(sys.argv[1])); print(data["total_game_count"], len(data["systems"]))' "$summary_copy"
)
assert_eq "launcher-visible game count" "$EXPECTED_VISIBLE_GAMES" "$visible_games"
assert_eq "launcher system count" "$EXPECTED_SYSTEMS" "$visible_systems"
remote "test -s '$REMOTE_NAVIGATION'"
echo "ok: $REMOTE_NAVIGATION is present and non-empty"

console_pack_count="$(
  remote "find '$REMOTE_ASSETS' -maxdepth 1 -type f \\( -name '*-screenshots.mmlz4b' -o -name '*-screenshots-320x320.mmlz4b' \\) 2>/dev/null | wc -l" | last_number
)"
if [ "$console_pack_count" -gt 0 ]; then
  asset_entry_tables="$(
    db_scalar "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='asset_entries';"
  )"
  assert_eq "runtime-only screenshot asset table count" "0" "$asset_entry_tables"
fi

if [ "$(pack_exists "arcade-screenshots-320x320.mmlz4b")" = "yes" ]; then
  echo "ok: size-qualified arcade screenshot pack installed"
fi

if [ "$(pack_exists ".screenshot-media-state.json")" = "yes" ]; then
  size_state_count="$(
    remote "grep -c 'screenshots-320x320\\.mmlz4b' '$REMOTE_ASSETS/.screenshot-media-state.json' 2>/dev/null || true" | last_number
  )"
  if [ "${size_state_count:-0}" -gt 0 ]; then
    echo "ok: media state size-qualified local_path count = $size_state_count"
    cache_state_count="$(
      remote "grep -c 'cf_cache_status\\|content_length\\|effective_url' '$REMOTE_ASSETS/.screenshot-media-state.json' 2>/dev/null || true" | last_number
    )"
    assert_gt_zero "media state cache metadata count" "$cache_state_count"
  else
    echo "ok: media state present without size-qualified runtime downloads"
  fi
else
  echo "ok: media state not present; runtime downloader has not published packs on this device"
fi

progress_log_count="$(
  remote "grep -h 'screenshot_media_progress' /tmp/mister-magik-slint.log /tmp/mister-magik-launcher.log /media/fat/mister-magik/*.log 2>/dev/null | wc -l" | last_number
)"
if [ "${progress_log_count:-0}" -gt 0 ]; then
  echo "ok: screenshot media progress log rows = $progress_log_count"
else
  echo "ok: screenshot media progress log not captured in known log files"
fi

catalog_seed_count="$(
  remote "grep -h 'screenshot_media_catalog_ensure' /tmp/mister-magik-slint.log /tmp/mister-magik-launcher.log /media/fat/mister-magik/*.log 2>/dev/null | wc -l" | last_number
)"
if [ "${catalog_seed_count:-0}" -gt 0 ]; then
  echo "ok: cached catalog screenshot media ensure rows = $catalog_seed_count"
else
  echo "ok: cached catalog screenshot media ensure rows not captured in known log files"
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
