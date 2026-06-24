#!/usr/bin/env bash
# Measure warm launcher startup catalog timing without forcing a rebuild.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-warm-catalog.tsv"

LABEL=""
ITERATIONS=1
REPLACE_LABEL=0
DEPLOY=0

usage() {
  cat <<'EOF'
Usage: scripts/profile-warm-catalog-start.sh LABEL [--replace-label] [--iterations N] [--deploy-device]

Restarts the production launcher with the default catalog refresh policy and
records warm catalog startup timings from startup_timing log rows. It does not
force a rebuild and does not launch a core.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --replace-label) REPLACE_LABEL=1; shift ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    --deploy-device) DEPLOY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$LABEL" ]]; then
        echo "unexpected argument: $1" >&2
        usage >&2
        exit 2
      fi
      LABEL="$1"
      shift
      ;;
  esac
done

if [[ -z "$LABEL" ]]; then
  LABEL="warmcat-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "--iterations must be a positive integer" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR" "$HERE/build/warm-catalog"
if [[ ! -f "$TSV" ]]; then
  echo "label	iteration	first_frame_ms	first_frame_catalog_ready	catalog_cache_load_sync_ms	catalog_cache_load_sync_total_us	catalog_summary_load_ms	catalog_summary_load_us	catalog_bridge_systems_us	catalog_bridge_sync_us	full_catalog_ready_ms	full_catalog_ready_load_us	result" >"$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

if [[ "$DEPLOY" -eq 1 ]]; then
  "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher
fi

env_file="$(mktemp)"
cleanup() {
  rm -f "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

{
  printf 'export MISTER_CATALOG_REFRESH=default\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=home\n'
} >"$env_file"

echo "== warm catalog startup profile label=$LABEL iterations=$ITERATIONS"
for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
  local_log="$HERE/build/warm-catalog/${LABEL}-${iteration}.log"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "rm -f '$REMOTE_LOG'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
  sleep 7
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
  if [[ ! -s "$local_log" ]]; then
    echo "warm catalog benchmark failed; missing launcher log $local_log" >&2
    exit 1
  fi
  awk -F '\t' -v label="$LABEL" -v iteration="$iteration" '
    BEGIN { OFS = "\t" }
    function ms_for(event,   i, ms) {
      for (i = 1; i <= n; i++) if (name[i] == event) {
        ms = at_ms[i]; sub(/ms$/, "", ms); return ms + 0
      }
      return -1
    }
    function detail_for(event,   i) {
      for (i = 1; i <= n; i++) if (name[i] == event) return detail[i]
      return ""
    }
    function field(detail, key,   parts, i, kv) {
      split(detail, parts, " ")
      for (i in parts) {
        split(parts[i], kv, "=")
        if (kv[1] == key) return kv[2]
      }
      return ""
    }
    $1 == "startup_timing" {
      n++
      name[n] = $2
      at_ms[n] = $3
      detail[n] = $4
    }
    END {
      first = ms_for("first_frame")
      first_detail = detail_for("first_frame")
      first_ready = field(first_detail, "catalog_ready")
      sync_ms = ms_for("catalog_cache_load_sync")
      sync_detail = detail_for("catalog_cache_load_sync")
      sync_total = field(sync_detail, "total_us")
      summary_ms = ms_for("catalog_summary_load")
      summary_detail = detail_for("catalog_summary_load")
      summary_us = field(summary_detail, "elapsed_us")
      bridge_systems = field(detail_for("catalog_bridge_systems"), "elapsed_us")
      bridge_sync = field(detail_for("catalog_bridge_sync"), "elapsed_us")
      ready_ms = ms_for("library_ready")
      ready_detail = detail_for("library_ready")
      ready_load_us = field(ready_detail, "load_us")
      result = first >= 0 ? "ok" : "missing_first_frame"
      print label, iteration, first, first_ready, sync_ms, sync_total, summary_ms, summary_us, bridge_systems, bridge_sync, ready_ms, ready_load_us, result
    }
  ' "$local_log" >>"$TSV"
  tail -n 1 "$TSV"
done

echo "appended to $TSV"
