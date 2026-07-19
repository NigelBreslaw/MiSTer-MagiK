#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Validate the sole production Catalog V3 generation on a running MiSTer.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
source "$ROOT/scripts/lib/magik-layout.sh"
source "$ROOT/scripts/lib/bench-context-lib.sh"

LAYOUT=dev
SETTLE_SECS=5
LABEL="catalog-v3-acceptance-$(date -u +%Y%m%dT%H%M%SZ)"
REPLACE_LABEL=0
SELF_TEST=0

usage() {
  echo "usage: scripts/device-catalog-acceptance.sh [--layout dev|public] [--settle SECS] [--label LABEL] [--replace-label] [--self-test]"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --layout) LAYOUT="${2:?--layout needs dev or public}"; shift 2 ;;
    --settle) SETTLE_SECS="${2:?--settle needs seconds}"; shift 2 ;;
    --label) LABEL="${2:?--label needs a value}"; shift 2 ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --race-refresh)
      echo "ERROR: --race-refresh belonged to the removed V2 database acceptance" >&2
      exit 2
      ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

magik_layout_select "$LAYOUT"

field() {
  local row="$1" key="$2"
  printf '%s\n' "$row" | awk -F '\t' -v key="$key" '
    { for (i=1; i<=NF; i++) if ($i ~ ("^" key "=")) { sub("^" key "=", "", $i); print $i; exit } }
  '
}

if [ "$SELF_TEST" -eq 1 ]; then
  fixture=$'catalog_v3_summary_tsv\tvalid=1\tschema=1\tgeneration=7\tsystems=3\ttotal_games=42\tarcade_resident_games=9\tstate_discoveries=42\tarcade_roles=1\tconsole_roles=1\tcomputer_roles=1\tfingerprint=abc'
  [ "$(field "$fixture" total_games)" = 42 ]
  [ "$(field "$fixture" schema)" = 1 ]
  echo "device-catalog-acceptance self-test ok"
  exit 0
fi

[[ "$SETTLE_SECS" =~ ^[0-9]+$ ]] || { echo "--settle must be an integer" >&2; exit 2; }

OUT="$ROOT/build/catalog-v3-acceptance/$LABEL"
RESULTS="$OUT/results.tsv"
REPORT="$OUT/catalog-v3-inspect.tsv"
mkdir -p "$OUT"
printf 'check\tstatus\texpected\tactual\n' >"$RESULTS"

record() {
  printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" >>"$RESULTS"
}

fail() {
  record "$1" fail "${2:-pass}" "${3:-failed}"
  echo "FAIL: $1 expected=${2:-pass} actual=${3:-failed}" >&2
  exit 1
}

remote() { "$MISTER" run "$1"; }
last_number() { awk 'NF { value=$NF } END { gsub(/[^0-9]/, "", value); print value }'; }

binary_path="$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
deployed_sha256="$(bench_context_remote_sha256 "$MISTER" "$MISTER_MAGIK_BIN" || true)"
deployed_sha256="${deployed_sha256:-missing}"
if ! bench_context_require_binary_contract "$binary_path" "$deployed_sha256" ui release-device launcher; then
  fail "binary identity" "production ui build" "$deployed_sha256"
fi

sleep "$SETTLE_SECS"
launcher_count="$(remote "ps w | grep '[m]ister-magik-fb ui launcher' | wc -l" | last_number)"
[ "$launcher_count" = 1 ] || fail "launcher process count" 1 "$launcher_count"
record "launcher process count" pass 1 "$launcher_count"

remote "'$MISTER_MAGIK_BIN' catalog-v3-inspect" >"$REPORT"
summary="$(awk -F '\t' '$1 == "catalog_v3_summary_tsv" { print; exit }' "$REPORT")"
[ -n "$summary" ] || fail "V3 summary" present missing
for pair in valid:1 schema:1 arcade_roles:1; do
  key="${pair%%:*}" expected="${pair#*:}" actual="$(field "$summary" "$key")"
  [ "$actual" = "$expected" ] || fail "$key" "$expected" "$actual"
  record "$key" pass "$expected" "$actual"
done
for key in generation systems total_games arcade_resident_games state_discoveries console_roles computer_roles; do
  actual="$(field "$summary" "$key")"
  [[ "$actual" =~ ^[0-9]+$ ]] && [ "$actual" -gt 0 ] || fail "$key" ">0" "${actual:-missing}"
  record "$key" pass ">0" "$actual"
done

systems="$(field "$summary" systems)"
system_rows="$(awk -F '\t' '$1 == "catalog_v3_system_tsv" { count++ } END { print count+0 }' "$REPORT")"
[ "$system_rows" = "$systems" ] || fail "system shard rows" "$systems" "$system_rows"
record "system shard rows" pass "$systems" "$system_rows"

legacy_count="$(remote "for path in '$MISTER_MAGIK_APP_DIR/library.sqlite3' '$MISTER_MAGIK_APP_DIR/library.summary.json' '$MISTER_MAGIK_APP_DIR/library.nav.lz4b'; do [ -e \"\$path\" ] && echo \"\$path\"; done | wc -l" | last_number)"
[ "${legacy_count:-0}" = 0 ] || fail "legacy V2 artifacts" 0 "$legacy_count"
record "legacy V2 artifacts" pass 0 "$legacy_count"

printf 'catalog_v3_acceptance_tsv\tlabel=%s\tvalid=1\tgeneration=%s\tsystems=%s\ttotal_games=%s\tarcade_resident_games=%s\tstate_discoveries=%s\treport=%s\n' \
  "$LABEL" "$(field "$summary" generation)" "$systems" "$(field "$summary" total_games)" \
  "$(field "$summary" arcade_resident_games)" "$(field "$summary" state_discoveries)" "$REPORT"
