#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Cold direct-to-system turbo preview gate for screenshot-capable systems.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-turbo-profiles"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_NAV="/media/fat/mister-magik/library.nav.lz4b"
source "$HERE/scripts/preview-selection-lib.sh"

label="cold-turbo-preview-$(date -u +%Y%m%dT%H%M%SZ)"
secs="10"
systems_csv="arcade,neogeo,saturn"
skip_reboot="0"
require_pass="0"
skip_archive_warm="1"
env_file=""
self_test="0"

usage() {
  cat <<'EOF'
Usage: scripts/profile-cold-turbo-preview.sh [LABEL] [--secs N] [--systems CSV] [--skip-reboot] [--skip-archive-warm] [--require-pass] [--self-test]

Runs the launcher cold/direct-to-system, starts the turbo-hold benchmark
immediately after the first usable preview, and summarizes state-chart rows for
zero-miss preview coverage before the full screenshot archive is warmed.

Default systems: arcade,neogeo,saturn.
By default this is reporting-only. Use --require-pass to fail on any miss.
EOF
}

positionals=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?--secs needs a value}"; shift 2 ;;
    --systems) systems_csv="${2:?--systems needs a value}"; shift 2 ;;
    --skip-reboot) skip_reboot="1"; shift ;;
    --skip-archive-warm) skip_archive_warm="1"; shift ;;
    --require-pass) require_pass="1"; shift ;;
    --self-test) self_test="1"; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done
if [[ "${#positionals[@]}" -ge 1 ]]; then label="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -gt 1 ]]; then usage >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$secs" =~ ^[0-9]+$ || "$secs" -lt 1 ]]; then
  echo "--secs must be a positive integer" >&2
  exit 2
fi

run_self_test() {
  preview_selection_self_test
  echo "profile-cold-turbo-preview self-test ok"
}

if [[ "$self_test" == "1" ]]; then
  run_self_test
  exit 0
fi

mkdir -p "$OUT_DIR"

reset_navigation_projection_for_benchmark() {
  local report
  report="$("$MISTER" run "
path='$REMOTE_NAV'
if [ -f \"\$path\" ]; then
  bytes=\$(wc -c < \"\$path\" 2>/dev/null || echo 0)
  rm -f \"\$path\"
  state=removed
else
  bytes=0
  state=missing
fi
sync
printf 'artifact_reset_tsv\t%s\t%s\t%s\t%s\n' '$label' \"\$state\" \"\$path\" \"\$bytes\"
")"
  printf '%s\n' "$report" | tee "$OUT_DIR/${label}-artifact-reset.tsv"
}

repair_navigation_projection_for_benchmark() {
  local report
  report="$("$MISTER" run "/media/fat/mister-magik/mister-magik-fb repair-catalog-projections")"
  printf '%s\n' "$report" | tee "$OUT_DIR/${label}-projection-repair.tsv"
}

write_env_for_system() {
  local system="$1"
  local selected_index="$2"
  env_file="$OUT_DIR/${label}-${system}.launcher.env"
  {
    printf 'export MISTER_CATALOG_REFRESH=default\n'
    printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
    printf 'export MISTER_LAUNCHER_START_SYSTEM=%q\n' "$system"
    printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=turbo-hold\n'
    printf 'export MISTER_PREVIEW_TRACE=1\n'
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
    printf 'export MISTER_PREVIEW_RUN_LABEL=%q\n' "$label"
    printf 'export MISTER_ARCADE_SELECTED_INDEX=%q\n' "$selected_index"
    printf 'export MISTER_PREVIEW_DECODED_CACHE_CAP=96\n'
    printf 'export MISTER_PREVIEW_TURBO_RUNWAY=1\n'
    if [[ "$skip_archive_warm" == "1" ]]; then
      printf 'export MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM=1\n'
    fi
  } >"$env_file"
  "$MISTER" run "mkdir -p /media/fat/mister-magik; rm -f '$REMOTE_LOG'" >/dev/null
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "sync" >/dev/null
}

cleanup() {
  if [[ -n "$env_file" ]]; then
    rm -f "$env_file"
  fi
  "$MISTER" run "rm -f '$REMOTE_ENV'; sync; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

summarize_log() {
  local target_system="$1" log="$2"
  awk -v label="$label" -v target_system="$target_system" '
    function field(name, fallback,    i, kv) {
      for (i = 4; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == name) return kv[2]
      }
      return fallback
    }
    function ms_value() {
      value = $3
      sub(/ms$/, "", value)
      return value + 0
    }
    BEGIN {
      FS = "\t"
      system_entered = list_ready = first_request = first_apply = archive_ready = full_catalog = -1
      selections = previewable = misses = stale = blank = exact = cache_hits = index_preads = selected_sync = archive_mem = 0
      max_request_to_apply = -1
      last_load_source = "unknown"
    }
    $1 == "startup_timing" && ($2 == "catalog_navigation_load" || $2 == "library_ready") && full_catalog < 0 { full_catalog = ms_value() }
    $1 == "startup_timing" && $2 == "preview_archive_warm" && archive_ready < 0 { archive_ready = ms_value() }
    $1 == "startup_timing" && $2 == "preview_system_entered" && field("system", "") == target_system && system_entered < 0 { system_entered = ms_value() }
    $1 == "startup_timing" && $2 == "preview_initial_list_ready" && field("system", "") == target_system && list_ready < 0 { list_ready = ms_value() }
    $1 == "startup_timing" && $2 == "preview_selected_requested" && field("system", "") == target_system {
      if (first_request < 0) first_request = ms_value()
      request_key = field("asset_key", "") "|" field("selected_index", "") "|" field("generation", "")
      request_ms[request_key] = ms_value()
    }
    $1 == "startup_timing" && $2 == "preview_selected_decoded" && field("system", "") == target_system {
      source = field("load_source", "unknown")
      last_load_source = source
      if (source == "index_pread") index_preads++
      if (source == "decoded_cache") cache_hits++
      if (source == "archive_mem") archive_mem++
      if (field("generation", "0") == "0" && source == "index_pread") selected_sync++
    }
    $1 == "startup_timing" && $2 == "preview_selected_applied" && field("system", "") == target_system {
      if (first_apply < 0) first_apply = ms_value()
      key = field("asset_key", "") "|" field("selected_index", "") "|" field("generation", "")
      if (key in request_ms) {
        delta = ms_value() - request_ms[key]
        if (delta > max_request_to_apply) max_request_to_apply = delta
      }
    }
    $1 == "startup_timing" && ($2 == "preview_selection_sample" || $2 == "preview_visible_exact" || $2 == "preview_visible_stale" || $2 == "preview_visible_blank" || $2 == "preview_miss") && field("system", "") == target_system && field("turbo_active", "0") == "1" {
      if ($2 != "preview_miss") {
        selections++
        if (field("has_preview", "0") == "1") previewable++
        state = field("cache_state", "")
        if (state == "exact") exact++
        if (state == "stale" || state == "cached" || state == "pending") stale++
        if (state == "blank" || state == "failed") blank++
      } else {
        misses++
      }
    }
    END {
      request_to_apply = (first_request >= 0 && first_apply >= 0) ? first_apply - first_request : -1
      before_archive_ok = (archive_ready < 0 && archive_mem == 0) || archive_ready >= 0
      pass = (system_entered >= 0 && list_ready >= 0 && previewable > 0 && misses == 0 && before_archive_ok) ? 1 : 0
      printf "preview_turbo_tsv\tlabel=%s\tsystem=%s\tsystem_entered_ms=%d\tinitial_list_ready_ms=%d\tfirst_request_ms=%d\tfirst_apply_ms=%d\tfirst_request_to_apply_ms=%d\tmax_request_to_apply_ms=%d\tfull_catalog_ms=%d\tarchive_ready_ms=%d\tselections=%d\tpreviewable_selections=%d\texact=%d\tstale=%d\tblank=%d\tmiss_count=%d\tdecoded_cache_hits=%d\tindex_pread_loads=%d\tselected_sync_index_preads=%d\tarchive_mem_loads=%d\tlast_load_source=%s\tpass=%d\n",
        label, target_system, system_entered, list_ready, first_request, first_apply,
        request_to_apply, max_request_to_apply, full_catalog, archive_ready, selections,
        previewable, exact, stale, blank, misses, cache_hits, index_preads, selected_sync,
        archive_mem, last_load_source, pass
    }
  ' "$log"
}

IFS=',' read -r -a systems <<<"$systems_csv"
all_pass="1"
reset_navigation_projection_for_benchmark
repair_navigation_projection_for_benchmark
for system in "${systems[@]}"; do
  local_log="$OUT_DIR/${label}-${system}.log"
  selected_index="$(preview_selection_index_for_system "$MISTER" "$system")" || {
    echo "no preview-bearing row found for system=$system in launcher_catalog" >&2
    exit 1
  }
  printf 'preview_turbo_start_tsv\tlabel=%s\tsystem=%s\tselected_index=%s\n' "$label" "$system" "$selected_index"
  write_env_for_system "$system" "$selected_index"
  if [[ "$skip_reboot" == "1" ]]; then
    "$MISTER" run "if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null
  else
    "$MISTER" reboot-wait
  fi
  sleep "$((secs + 8))"
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null
  summary="$(summarize_log "$system" "$local_log")"
  printf '%s\n' "$summary"
  if [[ "$summary" != *$'\tpass=1' ]]; then
    all_pass="0"
  fi
done

if [[ "$require_pass" == "1" && "$all_pass" != "1" ]]; then
  echo "cold turbo preview gate failed for one or more systems" >&2
  exit 1
fi
