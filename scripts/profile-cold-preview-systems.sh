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
self_test="0"

usage() {
  cat <<'EOF'
Usage: scripts/profile-cold-preview-systems.sh [LABEL] [--secs N] [--systems CSV] [--skip-reboot] [--require-pass] [--max-request-to-apply-ms N] [--self-test]

Runs the launcher from Home for each requested system and summarizes the
state-chart startup_timing rows for target-list readiness, preview-candidate
discovery, and the selected request/decode/apply path.

Default systems: arcade,neogeo,saturn.
By default this is reporting-only. Use --require-pass to fail when any system
with a preview candidate misses the first-preview gate. Systems with no preview
candidate are reported as explicit skips rather than latency passes.
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
      candidate_has_preview = "unknown"
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
      candidate_has_preview = field("has_preview", "unknown")
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
      readiness_present = (system_entered >= 0 && list_ready >= 0 && candidate >= 0)
      preview_ordered = (request >= candidate && decoded >= request && applied >= decoded)
      preview_pass = (candidate_has_preview == 1 && asset_key != "" && readiness_present && preview_ordered && request_to_apply_ms <= max_request_to_apply_ms && load_source == "index_pread")
      absent_candidate = (candidate_has_preview == 0 && readiness_present && request < 0 && decoded < 0 && applied < 0)
      if (preview_pass) {
        result = "pass"
        skip_reason = "none"
        failure_reason = "none"
        pass = 1
      } else if (absent_candidate) {
        result = "skip"
        skip_reason = "no_preview_candidate"
        failure_reason = "none"
        pass = 1
      } else {
        result = "fail"
        skip_reason = "none"
        pass = 0
        if (system_entered < 0) failure_reason = "missing_system_entered"
        else if (list_ready < 0) failure_reason = "missing_target_list_ready"
        else if (candidate < 0) failure_reason = "missing_candidate_discovery"
        else if (candidate_has_preview == 0) failure_reason = "unexpected_preview_phase_without_candidate"
        else if (candidate_has_preview != 1) failure_reason = "missing_candidate_availability"
        else if (asset_key == "") failure_reason = "missing_candidate_asset_key"
        else if (request < 0) failure_reason = "missing_selected_request"
        else if (decoded < 0) failure_reason = "missing_selected_decode"
        else if (applied < 0) failure_reason = "missing_selected_apply"
        else if (!preview_ordered) failure_reason = "misordered_preview_phases"
        else if (request_to_apply_ms > max_request_to_apply_ms) failure_reason = "request_to_apply_budget"
        else if (load_source != "index_pread") failure_reason = "unexpected_load_source"
        else failure_reason = "unknown"
      }
      printf "preview_state_tsv\tlabel=%s\tsystem=%s\tfirst_frame_ms=%d\tinput_enabled_ms=%d\tsystem_entered_ms=%d\ttarget_list_ready_ms=%d\tcandidate_discovered_ms=%d\tcandidate_has_preview=%d\tselected_has_preview=%s\tselected_request_ms=%d\tselected_decode_ms=%d\tselected_apply_ms=%d\trequest_to_apply_ms=%d\tpack_current_ms=%d\tfull_catalog_ms=%d\tload_source=%s\ttotal_us=%d\tread_us=%d\tdecode_us=%d\tage_us=%d\tcandidate_asset_key=%s\tcandidate_title=%s\tmax_request_to_apply_ms=%d\tresult=%s\tskip_reason=%s\tfailure_reason=%s\tpass=%d\n",
        label, target_system, first_frame, input_enabled, system_entered, list_ready, candidate,
        candidate_has_preview, selected_has_preview, request, decoded, applied, request_to_apply_ms, pack_current,
        full_catalog, load_source, total_us, read_us, decode_us, age_us, asset_key,
        candidate_title, max_request_to_apply_ms, result, skip_reason, failure_reason, pass
    }
  ' "$log"
}

run_self_test() {
  local test_dir pass_log skip_log misordered_log missing_log summary
  test_dir="$(mktemp -d "${TMPDIR:-/tmp}/cold-preview-self-test.XXXXXX")"
  pass_log="$test_dir/pass.log"
  skip_log="$test_dir/skip.log"
  misordered_log="$test_dir/misordered.log"
  missing_log="$test_dir/missing.log"

  printf '%s\n' \
    $'startup_timing\tpreview_system_entered\t10ms\tsystem=test' \
    $'startup_timing\tpreview_initial_list_ready\t20ms\tsystem=test' \
    $'startup_timing\tpreview_selected_candidate\t21ms\tsystem=test\ttitle=Nearby\thas_preview=1\tasset_key=test/nearby\tcandidate_index=1\tselected_has_preview=0' \
    $'startup_timing\tpreview_selected_requested\t22ms\tsystem=test' \
    $'startup_timing\tpreview_selected_decoded\t30ms\tsystem=test\tload_source=index_pread\ttotal_us=8000\tread_us=5000\tdecode_us=3000\tage_us=8000' \
    $'startup_timing\tpreview_selected_applied\t40ms\tsystem=test' >"$pass_log"
  summary="$(summarize_log test "$pass_log")"
  [[ "$summary" == *$'\tcandidate_has_preview=1\tselected_has_preview=0\t'* ]]
  [[ "$summary" == *$'\tresult=pass\t'* ]]

  printf '%s\n' \
    $'startup_timing\tpreview_system_entered\t10ms\tsystem=test' \
    $'startup_timing\tpreview_initial_list_ready\t20ms\tsystem=test' \
    $'startup_timing\tpreview_selected_candidate\t21ms\tsystem=test\ttitle=None\thas_preview=0\tasset_key=\tcandidate_index=\tselected_has_preview=0' >"$skip_log"
  summary="$(summarize_log test "$skip_log")"
  [[ "$summary" == *$'\tresult=skip\tskip_reason=no_preview_candidate\t'* ]]
  [[ "$summary" == *$'\tpass=1' ]]

  printf '%s\n' \
    $'startup_timing\tpreview_system_entered\t10ms\tsystem=test' \
    $'startup_timing\tpreview_initial_list_ready\t20ms\tsystem=test' \
    $'startup_timing\tpreview_selected_candidate\t21ms\tsystem=test\ttitle=Game\thas_preview=1\tasset_key=test/game\tcandidate_index=0\tselected_has_preview=1' \
    $'startup_timing\tpreview_selected_requested\t19ms\tsystem=test' \
    $'startup_timing\tpreview_selected_decoded\t30ms\tsystem=test\tload_source=index_pread' \
    $'startup_timing\tpreview_selected_applied\t40ms\tsystem=test' >"$misordered_log"
  summary="$(summarize_log test "$misordered_log")"
  [[ "$summary" == *$'\tresult=fail\t'* ]]
  [[ "$summary" == *$'\tfailure_reason=misordered_preview_phases\t'* ]]

  printf '%s\n' \
    $'startup_timing\tpreview_system_entered\t10ms\tsystem=test' \
    $'startup_timing\tpreview_initial_list_ready\t20ms\tsystem=test' \
    $'startup_timing\tpreview_selected_candidate\t21ms\tsystem=test\ttitle=Game\thas_preview=1\tasset_key=test/game\tcandidate_index=0\tselected_has_preview=1' \
    $'startup_timing\tpreview_selected_requested\t22ms\tsystem=test' \
    $'startup_timing\tpreview_selected_applied\t40ms\tsystem=test' >"$missing_log"
  summary="$(summarize_log test "$missing_log")"
  [[ "$summary" == *$'\tresult=fail\t'* ]]
  [[ "$summary" == *$'\tfailure_reason=missing_selected_decode\t'* ]]

  rm -rf "$test_dir"
  echo "profile-cold-preview-systems self-test ok"
}

if [[ "$self_test" == "1" ]]; then
  run_self_test
  exit 0
fi

cleanup() {
  if [[ -n "$env_file" ]]; then
    rm -f "$env_file"
  fi
  "$MISTER" run "rm -f '$REMOTE_ENV'; sync; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

IFS=',' read -r -a systems <<<"$systems_csv"
requested="0"
passed="0"
skipped="0"
failed="0"
for system in "${systems[@]}"; do
  [[ "$system" =~ ^[A-Za-z0-9_.-]+$ ]] || { echo "invalid system id: $system" >&2; exit 2; }
  requested="$((requested + 1))"
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
  case "$summary" in
    *$'\tresult=pass\t'*) passed="$((passed + 1))" ;;
    *$'\tresult=skip\t'*) skipped="$((skipped + 1))" ;;
    *) failed="$((failed + 1))" ;;
  esac
done

aggregate_pass="1"
if [[ "$failed" -ne 0 ]]; then
  aggregate_pass="0"
fi
printf 'preview_state_aggregate_tsv\tlabel=%s\trequested=%d\tpassed=%d\tskipped=%d\tfailed=%d\tpass=%d\n' \
  "$label" "$requested" "$passed" "$skipped" "$failed" "$aggregate_pass"

if [[ "$require_pass" == "1" && "$failed" -ne 0 ]]; then
  echo "cold preview gate failed for one or more systems" >&2
  exit 1
fi
