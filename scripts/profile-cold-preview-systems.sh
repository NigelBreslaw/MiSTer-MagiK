#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Cold-reboot first-preview state chart for screenshot-capable systems.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/magik-layout.sh"
magik_layout_select dev
OUT_DIR="$HERE/build/preview-state-profiles"
REMOTE_ENV="$MISTER_MAGIK_LAUNCHER_ENV"
REMOTE_LOG="/tmp/mister-magik-slint.log"

label="cold-preview-$(date -u +%Y%m%dT%H%M%SZ)"
secs="22"
systems_csv="arcade,neogeo,saturn"
skip_reboot="0"
require_pass="0"
max_request_to_apply_ms="32"
env_file=""

usage() {
  cat <<'EOF'
Usage: scripts/profile-cold-preview-systems.sh [LABEL] [--secs N] [--systems CSV] [--skip-reboot] [--require-pass] [--max-request-to-apply-ms N]

Runs the launcher from Home for each requested system and summarizes the
state-chart startup_timing rows for first list + first preview readiness.

Default systems: arcade,neogeo,saturn.
By default this is reporting-only. Use --require-pass to fail when any system
misses the first-preview gate.
EOF
}

positionals=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?--secs needs a value}"; shift 2 ;;
    --systems) systems_csv="${2:?--systems needs a value}"; shift 2 ;;
    --skip-reboot) skip_reboot="1"; shift ;;
    --require-pass) require_pass="1"; shift ;;
    --max-request-to-apply-ms) max_request_to_apply_ms="${2:?--max-request-to-apply-ms needs a value}"; shift 2 ;;
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
if [[ ! "$max_request_to_apply_ms" =~ ^[0-9]+$ || "$max_request_to_apply_ms" -lt 1 ]]; then
  echo "--max-request-to-apply-ms must be a positive integer" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"

write_env_for_system() {
  local system="$1"
  env_file="$OUT_DIR/${label}-${system}.launcher.env"
  {
    printf 'export MISTER_CATALOG_REFRESH=default\n'
    # Select by canonical catalog id after navigation hydration. Fixed Home
    # tile offsets became invalid when hierarchical taxonomy landed.
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_START_SYSTEM=%q\n' "$system"
    printf 'export MISTER_PREVIEW_TRACE=1\n'
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
    printf 'export MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM=1\n'
  } >"$env_file"
  "$MISTER" run "mkdir -p '$MISTER_MAGIK_APP_DIR'; rm -f '$REMOTE_LOG'" >/dev/null
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
  awk -v label="$label" -v target_system="$target_system" -v max_request_to_apply_ms="$max_request_to_apply_ms" '
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
      first_frame = input_enabled = system_entered = list_ready = candidate = request = decoded = applied = pack_current = full_catalog = -1
      load_source = "unknown"
      selected_has_preview = "unknown"
      candidate_title = ""
      asset_key = ""
      total_us = read_us = decode_us = age_us = -1
    }
    $1 == "startup_timing" && $2 == "first_frame" && first_frame < 0 { first_frame = ms_value() }
    $1 == "startup_timing" && $2 == "launcher_input_enabled" && input_enabled < 0 { input_enabled = ms_value() }
    $1 == "startup_timing" && ($2 == "catalog_navigation_load" || $2 == "library_ready") && full_catalog < 0 { full_catalog = ms_value() }
    $1 == "startup_timing" && $2 == "screenshot_media_pack_status" && field("system", "") == target_system && pack_current < 0 { pack_current = ms_value() }
    $1 == "startup_timing" && $2 == "preview_system_entered" && field("system", "") == target_system && system_entered < 0 { system_entered = ms_value() }
    $1 == "startup_timing" && $2 == "preview_initial_list_ready" && field("system", "") == target_system && list_ready < 0 { list_ready = ms_value() }
    $1 == "startup_timing" && $2 == "preview_selected_candidate" && field("system", "") == target_system && candidate < 0 {
      candidate = ms_value()
      selected_has_preview = field("selected_has_preview", "unknown")
      asset_key = field("asset_key", "")
      candidate_title = field("title", "")
    }
    $1 == "startup_timing" && $2 == "preview_selected_requested" && field("system", "") == target_system && request < 0 { request = ms_value() }
    $1 == "startup_timing" && $2 == "preview_selected_decoded" && field("system", "") == target_system && decoded < 0 {
      decoded = ms_value()
      load_source = field("load_source", "unknown")
      total_us = field("total_us", "-1") + 0
      read_us = field("read_us", "-1") + 0
      decode_us = field("decode_us", "-1") + 0
      age_us = field("age_us", "-1") + 0
    }
    $1 == "startup_timing" && $2 == "preview_selected_applied" && field("system", "") == target_system && applied < 0 { applied = ms_value() }
    END {
      request_to_apply_ms = (request >= 0 && applied >= 0) ? applied - request : -1
      preview_pass = (selected_has_preview == 1 && request >= 0 && decoded >= 0 && applied >= 0 && request_to_apply_ms >= 0 && request_to_apply_ms <= max_request_to_apply_ms && load_source == "index_pread")
      no_candidate_pass = (selected_has_preview == 0 && request < 0 && decoded < 0 && applied < 0)
      pass = (system_entered >= 0 && list_ready >= 0 && candidate >= 0 && (preview_pass || no_candidate_pass)) ? 1 : 0
      printf "preview_state_tsv\tlabel=%s\tsystem=%s\tfirst_frame_ms=%d\tinput_enabled_ms=%d\tsystem_entered_ms=%d\tinitial_list_ready_ms=%d\tselected_candidate_ms=%d\tselected_has_preview=%s\tpreview_requested_ms=%d\tpreview_decoded_ms=%d\tpreview_applied_ms=%d\trequest_to_apply_ms=%d\tpack_current_ms=%d\tfull_catalog_ms=%d\tload_source=%s\ttotal_us=%d\tread_us=%d\tdecode_us=%d\tage_us=%d\tasset_key=%s\tcandidate_title=%s\tmax_request_to_apply_ms=%d\tpass=%d\n",
        label, target_system, first_frame, input_enabled, system_entered, list_ready, candidate,
        selected_has_preview, request, decoded, applied, request_to_apply_ms, pack_current,
        full_catalog, load_source, total_us, read_us, decode_us, age_us, asset_key,
        candidate_title, max_request_to_apply_ms, pass
    }
  ' "$log"
}

IFS=',' read -r -a systems <<<"$systems_csv"
all_pass="1"
for system in "${systems[@]}"; do
  [[ "$system" =~ ^[A-Za-z0-9_.-]+$ ]] || { echo "invalid system id: $system" >&2; exit 2; }
  local_log="$OUT_DIR/${label}-${system}.log"
  write_env_for_system "$system"
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
  echo "cold preview gate failed for one or more systems" >&2
  exit 1
fi
