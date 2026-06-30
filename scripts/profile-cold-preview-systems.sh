#!/usr/bin/env bash
# Cold-reboot first-preview state chart for screenshot-capable systems.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-state-profiles"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"

label="cold-preview-$(date -u +%Y%m%dT%H%M%SZ)"
secs="22"
systems_csv="arcade,neogeo,saturn"
skip_reboot="0"

usage() {
  cat <<'EOF'
Usage: scripts/profile-cold-preview-systems.sh [LABEL] [--secs N] [--systems CSV] [--skip-reboot]

Runs the launcher from Home for each requested system and summarizes the
state-chart startup_timing rows for first list + first preview readiness.

Default systems: arcade,neogeo,saturn.
EOF
}

positionals=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?--secs needs a value}"; shift 2 ;;
    --systems) systems_csv="${2:?--systems needs a value}"; shift 2 ;;
    --skip-reboot) skip_reboot="1"; shift ;;
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

mkdir -p "$OUT_DIR"

input_script_for_system() {
  case "$1" in
    arcade) printf 'a' ;;
    neogeo) printf 'right,right,a' ;;
    saturn) printf 'right,right,right,right,right,a' ;;
    *) return 1 ;;
  esac
}

tsv_value() {
  printf '%s' "$1" | tr '\t\r\n' '   '
}

write_env_for_system() {
  local system="$1" input_script="$2" remote_trace="/tmp/${label}-${system}.tsv"
  "$MISTER" run "mkdir -p /media/fat/mister-magik; rm -f '$REMOTE_LOG' '$remote_trace'; printf '%s\n' 'export MISTER_CATALOG_REFRESH=default' 'export MISTER_LAUNCHER_START_SCREEN=home' 'export MISTER_LAUNCHER_INPUT_SCRIPT=$input_script' 'export MISTER_PREVIEW_TRACE=1' 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=$secs' 'export MISTER_PREVIEW_SCROLL_TRACE=$remote_trace' 'export MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM=1' > '$REMOTE_ENV'; sync" >/dev/null
}

cleanup() {
  "$MISTER" run "rm -f '$REMOTE_ENV'; sync; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

summarize_log() {
  local system="$1" log="$2"
  awk -v label="$label" -v system="$system" '
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
    $1 == "startup_timing" && $2 == "screenshot_media_pack_status" && field("system", "") == system && pack_current < 0 { pack_current = ms_value() }
    $1 == "startup_timing" && $2 == "preview_system_entered" && field("system", "") == system && system_entered < 0 { system_entered = ms_value() }
    $1 == "startup_timing" && $2 == "preview_initial_list_ready" && field("system", "") == system && list_ready < 0 { list_ready = ms_value() }
    $1 == "startup_timing" && $2 == "preview_selected_candidate" && field("system", "") == system && candidate < 0 {
      candidate = ms_value()
      selected_has_preview = field("has_preview", "unknown")
      asset_key = field("asset_key", "")
      candidate_title = field("title", "")
    }
    $1 == "startup_timing" && $2 == "preview_selected_requested" && field("system", "") == system && request < 0 { request = ms_value() }
    $1 == "startup_timing" && $2 == "preview_selected_decoded" && field("system", "") == system && decoded < 0 {
      decoded = ms_value()
      load_source = field("load_source", "unknown")
      total_us = field("total_us", "-1") + 0
      read_us = field("read_us", "-1") + 0
      decode_us = field("decode_us", "-1") + 0
      age_us = field("age_us", "-1") + 0
    }
    $1 == "startup_timing" && $2 == "preview_selected_applied" && field("system", "") == system && applied < 0 { applied = ms_value() }
    END {
      request_to_apply_ms = (request >= 0 && applied >= 0) ? applied - request : -1
      pass = (request_to_apply_ms >= 0 && request_to_apply_ms <= 32 && load_source == "index_pread") ? 1 : 0
      printf "preview_state_tsv\tlabel=%s\tsystem=%s\tfirst_frame_ms=%d\tinput_enabled_ms=%d\tsystem_entered_ms=%d\tinitial_list_ready_ms=%d\tselected_candidate_ms=%d\tselected_has_preview=%s\tpreview_requested_ms=%d\tpreview_decoded_ms=%d\tpreview_applied_ms=%d\trequest_to_apply_ms=%d\tpack_current_ms=%d\tfull_catalog_ms=%d\tload_source=%s\ttotal_us=%d\tread_us=%d\tdecode_us=%d\tage_us=%d\tasset_key=%s\tcandidate_title=%s\tpass=%d\n",
        label, system, first_frame, input_enabled, system_entered, list_ready, candidate,
        selected_has_preview, request, decoded, applied, request_to_apply_ms, pack_current,
        full_catalog, load_source, total_us, read_us, decode_us, age_us, asset_key,
        candidate_title, pass
    }
  ' "$log"
}

IFS=',' read -r -a systems <<<"$systems_csv"
for system in "${systems[@]}"; do
  input_script="$(input_script_for_system "$system")" || {
    echo "unsupported system for scripted benchmark: $system" >&2
    exit 2
  }
  local_log="$OUT_DIR/${label}-${system}.log"
  write_env_for_system "$system" "$input_script"
  if [[ "$skip_reboot" == "1" ]]; then
    "$MISTER" run "if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null
  else
    "$MISTER" reboot-wait
  fi
  sleep "$((secs + 8))"
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null
  summarize_log "$system" "$local_log"
done
