#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Reproduce and summarize first-boot screenshot media UI visibility.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/media-cold-boot"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-media-cold-boot.tsv"
REMOTE_ENV="/media/fat/mister-magik-dev/launcher.env"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-fb"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_DB="/media/fat/mister-magik-dev/library.sqlite3"
REMOTE_SUMMARY="/media/fat/mister-magik-dev/library.summary.json"
DEFAULT_MANIFEST_URL="https://assets.mistermagik.com/mister-magik/v1/manifest.json"
ORIGINAL_ARGS=("$@")
source "$HERE/scripts/lib/thread-sampler-lib.sh"
source "$HERE/scripts/lib/bench-context-lib.sh"
source "$HERE/scripts/lib/benchmark-cleanup-lib.sh"

label=""
deploy="skip"
replace_label=0
timeout_secs=900
timeout_explicit=0
manifest_url="${MISTER_MEDIA_MANIFEST_URL:-$DEFAULT_MANIFEST_URL}"
image_size="${MISTER_MEDIA_SIZE:-320x320}"
asset_dir=""
cleanup_assets=1
reset_catalog=1
self_test=0
systems_csv="arcade,neogeo,saturn"
deployed_sha256="missing"
arcade_trace_secs=0
arcade_scenario="human-turbo-hold"
contention_min_overlap_frames=300
contention_min_download_frames=180
contention_min_publish_frames=60
contention_min_selected_applies=10
contention_correctness_only=0

usage() {
  cat <<'EOF'
Usage: scripts/profile-media-cold-boot.sh LABEL [--deploy-device|--skip-build] [--replace-label] [--timeout SECS] [--asset-dir PATH] [--keep-assets] [--keep-catalog] [--manifest-url URL] [--image-size SIZE] [--thread-sample] [--arcade-trace-secs N] [--contention-correctness-only] [--self-test]

Runs the supervised launcher after a reboot with a cold catalog and screenshot
media asset directory, then emits AI-readable rows showing whether arcade,
neogeo, and saturn were discovered, ensured, queued, downloaded, and visible in
the media progress UI model.

The default asset directory is a label-scoped temporary path under
/media/fat/mister-magik-dev and is removed after the run. Use --keep-assets to
preserve it for inspection. Use --keep-catalog to reuse the installed catalog
database instead of forcing the first-boot scan path.
--thread-sample records /proc per-thread CPU/core/scheduler samples once per
second after reboot while the cold media run completes.
--arcade-trace-secs runs a human-turbo Arcade trace in the same launcher and
requires same-clock, per-operation overlap with real media work. Contention
runs always enable thread sampling and require --keep-catalog.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --deploy-device) deploy="device"; shift ;;
    --skip-build) deploy="skip"; shift ;;
    --replace-label) replace_label=1; shift ;;
    --timeout) timeout_secs="${2:?}"; timeout_explicit=1; shift 2 ;;
    --asset-dir) asset_dir="${2:?}"; cleanup_assets=0; shift 2 ;;
    --keep-assets) cleanup_assets=0; shift ;;
    --keep-catalog) reset_catalog=0; shift ;;
    --manifest-url) manifest_url="${2:?}"; shift 2 ;;
    --image-size) image_size="${2:?}"; shift 2 ;;
    --systems) systems_csv="${2:?}"; shift 2 ;;
    --thread-sample) thread_sample_enabled="1"; shift ;;
    --arcade-trace-secs) arcade_trace_secs="${2:?}"; shift 2 ;;
    --contention-correctness-only) contention_correctness_only=1; shift ;;
    --self-test) self_test=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$label" ]]; then
        echo "unexpected argument: $1" >&2
        usage >&2
        exit 2
      fi
      label="$1"
      shift
      ;;
  esac
done

if [[ -z "$label" ]]; then
  label="media-cold-boot-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$timeout_secs" =~ ^[0-9]+$ || "$timeout_secs" -lt 1 ]]; then
  echo "--timeout must be a positive integer number of seconds" >&2
  exit 2
fi
if [[ ! "$image_size" =~ ^[0-9]+x[0-9]+$ ]]; then
  echo "--image-size must look like 320x320" >&2
  exit 2
fi
if [[ ! "$arcade_trace_secs" =~ ^[0-9]+$ ]]; then
  echo "--arcade-trace-secs must be a non-negative integer" >&2
  exit 2
fi
if [[ "$arcade_trace_secs" -gt 0 ]]; then
  if [[ "$reset_catalog" -ne 0 ]]; then
    echo "--arcade-trace-secs requires --keep-catalog; use profile-media-arcade-contention.sh" >&2
    exit 2
  fi
  thread_sample_enabled="1"
  if [[ "$timeout_explicit" -eq 0 ]]; then
    timeout_secs=420
  fi
  if [[ "$timeout_secs" -gt 600 ]]; then
    echo "media contention --timeout is capped at 600 seconds" >&2
    exit 2
  fi
fi
if [[ "$contention_correctness_only" -eq 1 && "$arcade_trace_secs" -eq 0 ]]; then
  echo "--contention-correctness-only requires --arcade-trace-secs" >&2
  exit 2
fi
if [[ -z "$asset_dir" ]]; then
  asset_dir="/media/fat/mister-magik-dev/media-cold-boot-${label}-assets"
fi

mkdir -p "$OUT_DIR" "$BENCH_DIR"

tsv_value() {
  printf '%s' "$1" | tr '\t\r\n' '   '
}

shell_quote() {
  bench_context_shell_quote "$1"
}

file_sha256() {
  local path="$1"
  shasum -a 256 "$path" 2>/dev/null | awk '{ print $1 }'
}

png_stats() {
  local path="$1"
  python3 - "$path" <<'PY' 2>/dev/null || printf '0\t0\tunknown\n'
import struct
import sys
import zlib

path = sys.argv[1]
with open(path, "rb") as f:
    data = f.read()
if not data.startswith(b"\x89PNG\r\n\x1a\n"):
    raise SystemExit(1)
offset = 8
width = height = None
idat = bytearray()
while offset + 8 <= len(data):
    length = struct.unpack(">I", data[offset:offset + 4])[0]
    tag = data[offset + 4:offset + 8]
    payload = data[offset + 8:offset + 8 + length]
    offset += 12 + length
    if tag == b"IHDR":
        width, height, bit_depth, color_type = struct.unpack(">IIBB", payload[:10])
        if bit_depth != 8 or color_type != 6:
            raise SystemExit(1)
    elif tag == b"IDAT":
        idat.extend(payload)
    elif tag == b"IEND":
        break
if not width or not height:
    raise SystemExit(1)
raw = zlib.decompress(bytes(idat))
stride = width * 4
first = None
nonblank = False
for y in range(height):
    row = raw[y * (stride + 1):(y + 1) * (stride + 1)]
    if not row or row[0] != 0:
        raise SystemExit(1)
    pixels = row[1:]
    for x in range(0, len(pixels), 4):
        rgb = bytes(pixels[x:x + 3])
        if first is None:
            first = rgb
        elif rgb != first:
            nonblank = True
            break
    if nonblank:
        break
print(f"{width}\t{height}\t{str(nonblank).lower()}")
PY
}

emit_artifact_row() {
  local kind="$1" local_path="$2" remote_path="${3:-}"
  local exists="false" bytes="0" sha="" width="0" height="0" nonblank="unknown"
  if [[ -f "$local_path" ]]; then
    exists="true"
    bytes="$(wc -c <"$local_path" | tr -d ' ')"
    sha="$(file_sha256 "$local_path")"
    if [[ "$local_path" == *.png ]]; then
      IFS=$'\t' read -r width height nonblank <<<"$(png_stats "$local_path")"
    fi
  fi
  printf 'artifact_tsv\tlabel=%s\tkind=%s\tlocal_path=%s\tremote_path=%s\texists=%s\tbytes=%s\tsha256=%s\twidth=%s\theight=%s\tnonblank=%s\n' \
    "$(tsv_value "$label")" "$(tsv_value "$kind")" "$(tsv_value "$local_path")" \
    "$(tsv_value "$remote_path")" "$exists" "$bytes" "$sha" "$width" "$height" "$nonblank"
}

emit_validity_row() {
  local valid="$1" reason="$2" detail="${3:-}"
  printf 'validity_tsv\tlabel=%s\tvalid=%s\tinvalid_reason=%s\tdetail=%s\n' \
    "$(tsv_value "$label")" "$valid" "$(tsv_value "$reason")" "$(tsv_value "$detail")"
}

emit_contention_contract_row() {
  if [[ "$arcade_trace_secs" -le 0 ]]; then
    return 0
  fi
  printf 'media_arcade_contract_tsv\tlabel=%s\tmin_overlap_frames=%s\tmin_download_overlap_frames=%s\tmin_publish_overlap_frames=%s\tmin_selected_apply_rows=%s\tselected_apply_p99_us=250000\tpacing_p99_work_us=14500\tthread_gate=active-aligned\trationale=5s-total-3s-download-1s-publish-at-60hz\n' \
    "$label" "$contention_min_overlap_frames" "$contention_min_download_frames" \
    "$contention_min_publish_frames" "$contention_min_selected_applies"
}

emit_run_context_row() {
  local commit command_text started_at binary_path deployment_state binary_fields features runtime_type source_fields
  commit="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  command_text="scripts/profile-media-cold-boot.sh ${ORIGINAL_ARGS[*]}"
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  binary_path="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
  deployment_state="verified"
  features="ui"
  runtime_type="production"
  if [[ "$arcade_trace_secs" -gt 0 ]]; then
    features="ui,bench-tools"
    runtime_type="bench-tools"
  fi
  binary_fields="$(bench_context_binary_fields "release-device" "launcher" "$features" "$binary_path" "$runtime_type" "$deployment_state" "$deployed_sha256")"
  source_fields="$(bench_context_source_fields "$HERE")"
  if [[ "$thread_sample_enabled" == "1" ]]; then
    printf 'run_context_tsv\tlabel=%s\tcommit=%s\tcommand=%s\tdevice=mister\tsystems=%s\tasset_dir=%s\timage_size=%s\tdeploy=%s\treset_catalog=%s\ttimeout_secs=%s\tarcade_trace_secs=%s\tstarted_at=%s\t%s\t%s\tthread_sample=%s\n' \
      "$(tsv_value "$label")" "$commit" "$(tsv_value "$command_text")" \
      "$(tsv_value "$systems_csv")" "$(tsv_value "$asset_dir")" "$(tsv_value "$image_size")" \
      "$deploy" "$reset_catalog" "$timeout_secs" "$arcade_trace_secs" "$started_at" "$binary_fields" "$source_fields" "$thread_sample_enabled"
  else
    printf 'run_context_tsv\tlabel=%s\tcommit=%s\tcommand=%s\tdevice=mister\tsystems=%s\tasset_dir=%s\timage_size=%s\tdeploy=%s\treset_catalog=%s\ttimeout_secs=%s\tarcade_trace_secs=%s\tstarted_at=%s\t%s\t%s\n' \
      "$(tsv_value "$label")" "$commit" "$(tsv_value "$command_text")" \
      "$(tsv_value "$systems_csv")" "$(tsv_value "$asset_dir")" "$(tsv_value "$image_size")" \
      "$deploy" "$reset_catalog" "$timeout_secs" "$arcade_trace_secs" "$started_at" "$binary_fields" "$source_fields"
  fi
}

summarize_media_log() {
  local log_path="$1" summary_label="$2" commit="$3" wanted_systems="$4"
  awk -F '\t' -v label="$summary_label" -v commit="$commit" -v systems_csv="$wanted_systems" '
    BEGIN {
      count = split(systems_csv, list, ",")
      for (i = 1; i <= count; i++) {
        sys = list[i]
        wanted[sys] = 1
        order[++order_count] = sys
        terminal[sys] = "none"
        phases[sys] = "-"
        pack_statuses[sys] = "-"
        visible_systems[sys] = "-"
        ui_issue[sys] = "none"
        first_ms[sys] = ""
        last_ms[sys] = ""
      }
    }
    function kv(detail, key,   n, parts, i, item, value) {
      n = split(detail, parts, " ")
      for (i = 1; i <= n; i++) {
        item = parts[i]
        if (index(item, key "=") == 1) {
          value = item
          sub("^[^=]*=", "", value)
          return value
        }
      }
      return ""
    }
    function mark_time(sys, ms) {
      if (!(sys in wanted)) {
        return
      }
      if (first_ms[sys] == "") {
        first_ms[sys] = ms
      }
      last_ms[sys] = ms
    }
    function append_value(existing, value) {
      if (value == "") {
        return existing
      }
      if (existing == "" || existing == "-") {
        return value
      }
      if (index("," existing ",", "," value ",") > 0) {
        return existing
      }
      return existing "," value
    }
    $1 == "startup_timing" {
      event = $2
      ms = $3
      sub(/ms$/, "", ms)
      detail = $4
      for (i = 5; i <= NF; i++) {
        detail = detail " " $i
      }
      sys = kv(detail, "system")
      if (event == "screenshot_media_update_done") {
        update_done = 1
      }
      if (event == "screenshot_media_update_failed") {
        update_failed = 1
        update_failed_detail = detail
      }
      if (!(sys in wanted)) {
        next
      }
      mark_time(sys, ms)
      if (event == "catalog_system_discovered") {
        discovered[sys] = 1
      } else if (event == "screenshot_media_catalog_system_present") {
        discovered[sys] = 1
      } else if (event == "screenshot_media_catalog_ensure") {
        ensured[sys] = 1
      } else if (event == "screenshot_media_system_queued") {
        queued[sys] = 1
      } else if (event == "screenshot_media_system_start") {
        queue_started[sys] = 1
        pending_after_start[sys] = kv(detail, "pending")
      } else if (event == "screenshot_media_pack_status") {
        status = kv(detail, "status")
        pack_statuses[sys] = append_value(pack_statuses[sys], status)
      } else if (event == "screenshot_media_progress") {
        phase = kv(detail, "phase")
        phases[sys] = append_value(phases[sys], phase)
        if (phase ~ /^download/) {
          progress_download_seen[sys] = 1
        }
        if (phase == "done" || phase == "failed" || phase == "skipped-current" || phase == "check-only") {
          terminal[sys] = phase
        }
      } else if (event == "screenshot_media_ui_visibility") {
        if (kv(detail, "row_seen") == "1") {
          ui_row_seen[sys] = 1
        }
        if (kv(detail, "rendered") == "1") {
          ui_rendered_seen[sys] = 1
        }
        visible = kv(detail, "visible_systems")
        if (visible != "") {
          visible_systems[sys] = visible
        }
      }
    }
    END {
      all_terminal = 1
      for (i = 1; i <= order_count; i++) {
        if (terminal[order[i]] == "none") {
          all_terminal = 0
        }
      }
      complete = update_done || all_terminal
      completion = update_done ? "worker_done" : (all_terminal ? "targets_terminal" : "incomplete")
      for (i = 1; i <= order_count; i++) {
        sys = order[i]
        reason = "ok"
        if (update_failed) {
          reason = "media_worker_failed"
        } else if (!complete) {
          reason = "media_worker_incomplete"
        } else if (!discovered[sys]) {
          reason = "not_discovered"
        } else if (!ensured[sys]) {
          reason = "not_ensured"
        } else if (!queued[sys]) {
          reason = "not_queued"
        } else if (terminal[sys] == "none") {
          reason = "not_terminal"
        }
        issue = "none"
        if (!ui_row_seen[sys]) {
          issue = "row_missing"
        } else if (!ui_rendered_seen[sys]) {
          issue = "render_missing"
        }
        first = first_ms[sys]
        if (first == "") {
          first = "-"
        }
        last = last_ms[sys]
        if (last == "") {
          last = "-"
        }
        pending = pending_after_start[sys]
        if (pending == "") {
          pending = "-"
        }
        printf "media_cold_boot_tsv\tlabel=%s\tsystem=%s\tcommit=%s\tdiscovered=%d\tensured=%d\tqueued=%d\tqueue_started=%d\tprogress_download_seen=%d\tui_row_seen=%d\tui_rendered_seen=%d\tterminal=%s\tworker_done=%d\tcompletion=%s\tinvalid_reason=%s\tui_issue=%s\tfirst_ms=%s\tlast_ms=%s\tphases=%s\tpack_statuses=%s\tvisible_systems=%s\tpending_after_start=%s\n",
          label, sys, commit, discovered[sys] + 0, ensured[sys] + 0,
          queued[sys] + 0, queue_started[sys] + 0, progress_download_seen[sys] + 0,
          ui_row_seen[sys] + 0, ui_rendered_seen[sys] + 0, terminal[sys],
          update_done + 0, completion, reason, issue,
          first, last, phases[sys], pack_statuses[sys], visible_systems[sys], pending
        printf "metric_tsv\tlabel=%s\tsystem=%s\tmetric=media_cold_boot_ui_row_seen\tvalue=%d\tunit=bool\tvalid=%d\n",
          label, sys, ui_row_seen[sys] + 0, complete + 0
        printf "metric_tsv\tlabel=%s\tsystem=%s\tmetric=media_cold_boot_progress_download_seen\tvalue=%d\tunit=bool\tvalid=%d\n",
          label, sys, progress_download_seen[sys] + 0, complete + 0
      }
      if (update_failed) {
        printf "media_cold_boot_failure_tsv\tlabel=%s\tcommit=%s\tdetail=%s\n", label, commit, update_failed_detail
      }
    }
  ' "$log_path"
}

media_completion_report() {
  local log_path="$1" report_label="$2"
  python3 - "$log_path" "$report_label" "$systems_csv" <<'PY'
import collections
import sys

log_path, label, systems_csv = sys.argv[1:4]
required_systems = {item for item in systems_csv.split(",") if item}
queued = collections.Counter()
terminals = collections.Counter()
terminal_phase = {}
worker_done = []
worker_failed = 0
pack_failed = 0

def detail_fields(fields):
    result = {}
    for item in " ".join(fields).split():
        if "=" in item:
            key, value = item.split("=", 1)
            result[key] = value
    return result

with open(log_path, encoding="utf-8", errors="replace") as source:
    for line in source:
        fields = line.rstrip("\n").split("\t")
        if len(fields) < 4 or fields[0] != "startup_timing":
            continue
        event = fields[1]
        detail = detail_fields(fields[3:])
        system = detail.get("system", "")
        if event == "screenshot_media_system_queued":
            queued[(system, detail.get("pack_index", ""))] += 1
        elif event == "screenshot_media_progress":
            phase = detail.get("phase", "")
            if phase in {"done", "failed", "skipped-current", "check-only"}:
                key = (system, detail.get("pack_index", ""))
                terminals[key] += 1
                terminal_phase[key] = phase
        elif event == "screenshot_media_pack_status" and detail.get("status") == "failed":
            pack_failed += 1
        elif event == "screenshot_media_update_failed":
            worker_failed += 1
        elif event == "screenshot_media_update_done":
            worker_done.append(detail)

done_detail = worker_done[0] if len(worker_done) == 1 else {}
try:
    done_packs = int(done_detail.get("packs", "-1"))
    done_failed = int(done_detail.get("failed", "-1"))
except ValueError:
    done_packs = -1
    done_failed = -1

successful = {"done", "skipped-current", "check-only"}
queued_set = {key for key, count in queued.items() if count}
terminal_set = {key for key, count in terminals.items() if count}
queued_systems = {system for system, _pack_index in queued_set}
requested_packs = len(queued_set)
valid = (
    bool(queued_set)
    and required_systems.issubset(queued_systems)
    and len(worker_done) == 1
    and worker_failed == 0
    and pack_failed == 0
    and terminal_set == queued_set
    and all(count == 1 for count in queued.values())
    and all(terminals[key] == 1 for key in queued_set)
    and all(terminal_phase.get(key) in successful for key in queued_set)
    and done_packs == requested_packs
    and done_failed == 0
)
reasons = []
if not queued_set: reasons.append("no-queued-packs")
if not required_systems.issubset(queued_systems): reasons.append("required-systems")
if len(worker_done) != 1: reasons.append("worker-done-count")
if worker_failed: reasons.append("worker-failed")
if pack_failed: reasons.append("pack-failed")
if terminal_set != queued_set: reasons.append("terminal-set")
if any(count != 1 for count in queued.values()): reasons.append("queued-count")
if any(terminals[key] != 1 for key in queued_set): reasons.append("terminal-count")
if any(terminal_phase.get(key) not in successful for key in queued_set): reasons.append("terminal-phase")
if done_packs != requested_packs: reasons.append("done-packs")
if done_failed != 0: reasons.append("done-failed")
phase_detail = ",".join(
    f"{system}#{pack_index}:{terminal_phase.get((system, pack_index), 'missing')}"
    for system, pack_index in sorted(queued_set)
)
print(
    f"media_completion_tsv\tlabel={label}\tvalid={1 if valid else 0}"
    f"\tinvalid_reason={'ok' if valid else ','.join(reasons)}"
    f"\trequested_packs={requested_packs}\tqueued_packs={sum(queued.values())}"
    f"\tterminal_packs={sum(terminals.values())}\tworker_done_count={len(worker_done)}"
    f"\tdone_packs={done_packs}\tdone_failed={done_failed}"
    f"\tworker_failed_count={worker_failed}\tpack_failed_count={pack_failed}"
    f"\tterminals={phase_detail}"
)
raise SystemExit(0 if valid else 9)
PY
}

arcade_trace_complete() {
  local trace_path="$1" required_secs="$2"
  awk -F '\t' -v required_us="$((required_secs * 1000000))" '
    NR == 1 {
      for (i = 1; i <= NF; i++) if ($i == "elapsed_us") elapsed_col = i
      next
    }
    elapsed_col > 0 && $elapsed_col ~ /^[0-9]+$/ { latest = $elapsed_col }
    END { exit !(elapsed_col > 0 && latest >= required_us - 1000000) }
  ' "$trace_path"
}

media_arcade_contention_report() {
  local log_path="$1" trace_path="$2" thread_path="$3" report_label="$4" subset_path="$5"
  local min_overlap_frames="$6" min_download_frames="$7" min_publish_frames="$8" min_selected_applies="$9"
  python3 - "$log_path" "$trace_path" "$thread_path" "$report_label" "$subset_path" \
    "$min_overlap_frames" "$min_download_frames" "$min_publish_frames" "$min_selected_applies" <<'PY'
import collections
import csv
import math
import sys

log_path, trace_path, thread_path, label, subset_path = sys.argv[1:6]
min_overlap_frames, min_download_frames, min_publish_frames, min_selected_applies = map(int, sys.argv[6:10])

def detail_fields(fields):
    result = {}
    for item in " ".join(fields).split():
        if "=" in item:
            key, value = item.split("=", 1)
            result[key] = value
    return result

media_events = collections.defaultdict(list)
selected_apply = []
selected_failures = 0
with open(log_path, encoding="utf-8", errors="replace") as source:
    for line in source:
        fields = line.rstrip("\n").split("\t")
        if "preview_trace cache_failed" in line and ("selected=true" in line or "selected=1" in line):
            selected_failures += 1
        if len(fields) < 4 or fields[0] != "startup_timing":
            continue
        try:
            timestamp_us = int(fields[2].removesuffix("ms")) * 1000
        except ValueError:
            continue
        detail = detail_fields(fields[3:])
        if fields[1] == "screenshot_media_progress":
            system = detail.get("system", "")
            phase = detail.get("phase", "")
            pack_index = detail.get("pack_index", "0")
            if system and phase:
                media_events[(system, pack_index)].append((timestamp_us, phase))
        elif fields[1] == "preview_selected_applied":
            try:
                age_us = int(detail.get("age_us", ""))
            except ValueError:
                continue
            selected_apply.append((timestamp_us, age_us, detail.get("load_source", "unknown")))

operation_class = {
    "download_start": "download",
    "download": "download",
    "download_done": "publish",
    "verify": "publish",
    "save": "publish",
    "sync": "publish",
    "rename": "publish",
    "parent-sync": "publish",
}
intervals = []
for (system, pack_index), events in sorted(media_events.items()):
    for (start_us, phase), (end_us, _next_phase) in zip(events, events[1:]):
        kind = operation_class.get(phase)
        if kind and end_us > start_us:
            intervals.append((start_us, end_us, system, pack_index, phase, kind))

frames = []
trace_rows = []
trace_columns = []
with open(trace_path, newline="", encoding="utf-8") as source:
    reader = csv.DictReader(source, delimiter="\t")
    trace_columns = reader.fieldnames or []
    for row in reader:
        try:
            timestamp_us = int(row.get("startup_elapsed_us", ""))
            monotonic_us = int(row.get("monotonic_us", ""))
        except ValueError:
            continue
        trace_rows.append(row)
        frames.append((
            timestamp_us,
            monotonic_us,
            row.get("cache_state", ""),
            row.get("main_present_backend", ""),
            row.get("main_present_status", ""),
        ))

required_trace_columns = {
    "startup_elapsed_us",
    "monotonic_us",
    "main_present_backend",
    "main_present_status",
}
missing_trace_columns = sorted(required_trace_columns.difference(trace_columns))
if missing_trace_columns:
    print(
        f"media_arcade_overlap_tsv\tlabel={label}\tvalid=0"
        f"\tinvalid_reason=missing-required-trace-columns"
        f"\tmissing_columns={','.join(missing_trace_columns)}"
        f"\toperations={len(intervals)}\tframes={len(frames)}"
    )
    raise SystemExit(9)

origins = [monotonic_us - startup_us for startup_us, monotonic_us, *_rest in frames]
startup_origin_us = origins[0] if origins else 0
clock_consistent = bool(origins) and max(origins) - min(origins) <= 1000

def in_interval(timestamp_us):
    return any(start_us <= timestamp_us < end_us for start_us, end_us, *_ in intervals)

overlap_frame_indexes = set()
class_frame_indexes = collections.defaultdict(set)
overlapping_operations = 0
for operation_index, (start_us, end_us, system, pack_index, phase, kind) in enumerate(intervals, 1):
    indexes = {
        index for index, (timestamp_us, _monotonic_us, *_rest) in enumerate(frames)
        if start_us <= timestamp_us < end_us
    }
    overlap_frame_indexes.update(indexes)
    class_frame_indexes[kind].update(indexes)
    if indexes:
        overlapping_operations += 1
    print(
        f"media_arcade_operation_overlap_tsv\tlabel={label}\toperation={operation_index}"
        f"\tsystem={system}\tpack_index={pack_index}\tphase={phase}\tclass={kind}"
        f"\tstart_us={start_us}\tend_us={end_us}\tduration_us={end_us - start_us}"
        f"\tmonotonic_start_us={startup_origin_us + start_us}"
        f"\tmonotonic_end_us={startup_origin_us + end_us}"
        f"\toverlap_frames={len(indexes)}"
    )

with open(subset_path, "w", newline="", encoding="utf-8") as target:
    writer = csv.DictWriter(target, fieldnames=trace_columns, delimiter="\t", lineterminator="\n")
    writer.writeheader()
    writer.writerows(trace_rows[index] for index in sorted(overlap_frame_indexes))

overlap_count = len(overlap_frame_indexes)
download_overlap_count = len(class_frame_indexes["download"])
publish_overlap_count = len(class_frame_indexes["publish"])
overlap_valid = bool(
    clock_consistent
    and overlap_count >= min_overlap_frames
    and download_overlap_count >= min_download_frames
    and publish_overlap_count >= min_publish_frames
)
print(
    f"media_arcade_overlap_tsv\tlabel={label}\tvalid={1 if overlap_valid else 0}"
    f"\tinvalid_reason={'ok' if overlap_valid else 'no-per-operation-overlap'}"
    f"\tclock=startup_elapsed_us+monotonic_us\tclock_consistent={1 if clock_consistent else 0}"
    f"\tstartup_origin_us={startup_origin_us}\toperations={len(intervals)}"
    f"\toverlapping_operations={overlapping_operations}\tframes={len(frames)}"
    f"\toverlap_frames={overlap_count}\tmin_overlap_frames={min_overlap_frames}"
    f"\tdownload_overlap_frames={download_overlap_count}\tmin_download_frames={min_download_frames}"
    f"\tpublish_overlap_frames={publish_overlap_count}\tmin_publish_frames={min_publish_frames}"
)

overlap_states = collections.Counter(frames[index][2] for index in overlap_frame_indexes)
overlap_backends = collections.Counter(frames[index][3] for index in overlap_frame_indexes)
overlap_present_statuses = collections.Counter(frames[index][4] for index in overlap_frame_indexes)
presentation_valid = bool(
    overlap_frame_indexes
    and overlap_backends == collections.Counter({"fpga-vblank-latch-hidden": overlap_count})
    and overlap_present_statuses == collections.Counter({"ok": overlap_count})
)
print(
    f"media_arcade_presentation_tsv\tlabel={label}\tvalid={1 if presentation_valid else 0}"
    f"\tinvalid_reason={'ok' if presentation_valid else 'unstable-backend-or-status'}"
    f"\toverlap_frames={overlap_count}"
    f"\tbackend_fpga_vblank_latch_hidden={overlap_backends.get('fpga-vblank-latch-hidden', 0)}"
    f"\tstatus_ok={overlap_present_statuses.get('ok', 0)}"
    f"\tbackend_variants={len(overlap_backends)}\tstatus_variants={len(overlap_present_statuses)}"
)
invalid_states = sum(
    count for state, count in overlap_states.items() if state not in {"exact", "empty"}
)
overlap_applies = [(age, source) for timestamp, age, source in selected_apply if in_interval(timestamp)]
ages = sorted(age for age, _source in overlap_applies)
p99_age = ages[max(0, math.ceil(len(ages) * 0.99) - 1)] if ages else 0
sources = collections.Counter(source for _age, source in overlap_applies)
preview_valid = bool(
    overlap_frame_indexes
    and overlap_states.get("exact", 0) > 0
    and invalid_states == 0
    and len(ages) >= min_selected_applies
    and p99_age <= 250_000
    and selected_failures == 0
)
state_detail = ",".join(f"{key or 'blank'}:{value}" for key, value in sorted(overlap_states.items())) or "none"
source_detail = ",".join(f"{key}:{value}" for key, value in sorted(sources.items())) or "none"
print(
    f"media_arcade_preview_evidence_tsv\tlabel={label}\tvalid={1 if preview_valid else 0}"
    f"\tinvalid_reason={'ok' if preview_valid else 'missing-or-slow-selected-preview-evidence'}"
    f"\toverlap_frames={len(overlap_frame_indexes)}\texact_frames={overlap_states.get('exact', 0)}"
    f"\tempty_frames={overlap_states.get('empty', 0)}\tinvalid_cache_state_frames={invalid_states}"
    f"\tcache_states={state_detail}\tselected_apply_rows={len(ages)}\tmin_selected_apply_rows={min_selected_applies}"
    f"\tselected_apply_p99_us={p99_age}\tselected_apply_max_us={max(ages) if ages else 0}"
    f"\tselected_apply_sources={source_detail}\tselected_failures={selected_failures}"
)

thread_categories = {
    "launcher": lambda name: name == "mister-magik-fb",
    "media": lambda name: name.startswith("screenshot-medi"),
    "selected-preview": lambda name: name.startswith("preview-select"),
}
thread_evidence = {
    category: {"rows": 0, "aligned": 0, "active": 0, "samples": set(), "cores": set(), "cpu": 0}
    for category in thread_categories
}
overlap_monotonic_values = [frames[index][1] for index in overlap_frame_indexes]
overlap_monotonic_start = min(overlap_monotonic_values) if overlap_monotonic_values else 0
overlap_monotonic_end = max(overlap_monotonic_values) if overlap_monotonic_values else 0
try:
    with open(thread_path, newline="", encoding="utf-8") as source:
        for row in csv.DictReader(source, delimiter="\t"):
            name = row.get("thread_name", "")
            for category, matches in thread_categories.items():
                if not matches(name):
                    continue
                evidence = thread_evidence[category]
                evidence["rows"] += 1
                try:
                    interval_start = int(row.get("interval_start_monotonic_us", "") or 0)
                    interval_end = int(row.get("monotonic_us", "") or 0)
                    cpu = int(row.get("utime_delta_jiffies", "0") or 0) + int(row.get("stime_delta_jiffies", "0") or 0)
                except ValueError:
                    continue
                aligned = clock_consistent and any(
                    interval_start < startup_origin_us + end_us
                    and interval_end >= startup_origin_us + start_us
                    for start_us, end_us, *_ in intervals
                ) and interval_start <= overlap_monotonic_end and interval_end >= overlap_monotonic_start
                if not aligned:
                    continue
                evidence["aligned"] += 1
                evidence["samples"].add(row.get("sample", ""))
                evidence["cores"].add(row.get("processor", ""))
                evidence["cpu"] += cpu
                if cpu > 0:
                    evidence["active"] += 1
except FileNotFoundError:
    pass

thread_valid = True
for category, evidence in thread_evidence.items():
    valid = evidence["aligned"] > 0 and evidence["active"] > 0 and evidence["cpu"] > 0
    thread_valid = thread_valid and valid
    cores = ",".join(sorted(core for core in evidence["cores"] if core)) or "none"
    print(
        f"media_arcade_thread_evidence_tsv\tlabel={label}\tcategory={category}"
        f"\tvalid={1 if valid else 0}\tobservations={evidence['rows']}"
        f"\taligned_observations={evidence['aligned']}\tactive_aligned_observations={evidence['active']}"
        f"\taligned_samples={len(evidence['samples'])}\tcores={cores}"
        f"\tcpu_delta_jiffies={evidence['cpu']}"
    )
print(
    f"media_arcade_thread_gate_tsv\tlabel={label}\tvalid={1 if thread_valid else 0}"
    f"\tinvalid_reason={'ok' if thread_valid else 'missing-required-thread-evidence'}"
)

valid = overlap_valid and presentation_valid and preview_valid and thread_valid
print(
    f"media_arcade_contention_gate_tsv\tlabel={label}\tvalid={1 if valid else 0}"
    f"\tinvalid_reason={'ok' if valid else 'contention-evidence-incomplete'}"
)
raise SystemExit(0 if valid else 9)
PY
}

run_self_test() {
  local tmp log out trace thread_trace
  if "$0" timeout-cap-selftest --skip-build --keep-catalog \
    --arcade-trace-secs 1 --timeout 601 >/dev/null 2>&1; then
    echo "media contention self-test accepted timeout above the 600-second cap" >&2
    return 1
  fi
  tmp="$(mktemp -d)"
  log="$tmp/media.log"
  out="$tmp/out.tsv"
  trace="$tmp/arcade.tsv"
  cat >"$log" <<'EOF'
startup_timing	catalog_system_discovered	1ms	system=arcade
startup_timing	screenshot_media_catalog_system_present	2ms	system=arcade source=catalog-seed
startup_timing	screenshot_media_catalog_ensure	3ms	system=arcade
startup_timing	screenshot_media_system_queued	4ms	system=arcade pack_index=1 requested=1 pending=1
startup_timing	screenshot_media_system_start	5ms	system=arcade pack_index=1 pack_count=3 pending=2 active=1 max_concurrent=1 policy=download
startup_timing	screenshot_media_progress	6ms	system=arcade image_size=320x320 variant=identity phase=download_start bytes_done=0 bytes_total=100 percent=0 pack_index=1 pack_count=3 download_mbps= detail=
startup_timing	screenshot_media_ui_visibility	7ms	system=arcade row_seen=1 row_index=0 rendered=1 catalog_scan_visible=1 active_rows=1 visible_count=1 visible_systems=arcade phase=download percent=0 summary_active=1 summary_done=0 summary_failed=0 summary_total=3
startup_timing	screenshot_media_progress	8ms	system=arcade image_size=320x320 variant=identity phase=download bytes_done=50 bytes_total=100 percent=50 pack_index=1 pack_count=3 download_mbps= detail=
startup_timing	preview_selected_applied	11ms	system=arcade selected_index=4 title=1942 has_preview=1 asset_key=1942 generation=4 load_source=index_pread total_us=50000 read_us=1000 decode_us=49000 age_us=50000
startup_timing	screenshot_media_progress	12ms	system=arcade image_size=320x320 variant=identity phase=download_done bytes_done=100 bytes_total=100 percent=100 pack_index=1 pack_count=3 download_mbps= detail=
startup_timing	screenshot_media_progress	14ms	system=arcade image_size=320x320 variant=identity phase=save bytes_done=100 bytes_total=100 percent=100 pack_index=1 pack_count=3 download_mbps= detail=
startup_timing	screenshot_media_progress	18ms	system=arcade image_size=320x320 variant=identity phase=done bytes_done=100 bytes_total=100 percent=100 pack_index=1 pack_count=3 download_mbps= detail=
startup_timing	catalog_system_discovered	8ms	system=neogeo
startup_timing	screenshot_media_catalog_system_present	9ms	system=neogeo source=catalog-seed
startup_timing	screenshot_media_catalog_ensure	10ms	system=neogeo
startup_timing	screenshot_media_system_queued	11ms	system=neogeo pack_index=2 requested=2 pending=1
startup_timing	screenshot_media_system_start	12ms	system=neogeo pack_index=2 pack_count=3 pending=1 active=1 max_concurrent=1 policy=download
startup_timing	screenshot_media_progress	13ms	system=neogeo image_size=320x320 variant=identity phase=download_start bytes_done=0 bytes_total=100 percent=0 pack_index=2 pack_count=3 download_mbps= detail=
startup_timing	screenshot_media_ui_visibility	14ms	system=neogeo row_seen=0 row_index=-1 rendered=0 catalog_scan_visible=1 active_rows=4 visible_count=3 visible_systems=arcade,megadrive,n64 phase=download percent=0 summary_active=2 summary_done=1 summary_failed=0 summary_total=3
startup_timing	screenshot_media_progress	15ms	system=neogeo image_size=320x320 variant=identity phase=done bytes_done=100 bytes_total=100 percent=100 pack_index=2 pack_count=3 download_mbps= detail=
startup_timing	catalog_system_discovered	15ms	system=saturn
startup_timing	screenshot_media_catalog_system_present	16ms	system=saturn source=catalog-seed
startup_timing	screenshot_media_catalog_ensure	17ms	system=saturn
startup_timing	screenshot_media_system_queued	18ms	system=saturn pack_index=3 requested=3 pending=1
startup_timing	screenshot_media_system_start	19ms	system=saturn pack_index=3 pack_count=3 pending=0 active=1 max_concurrent=1 policy=download
startup_timing	screenshot_media_progress	20ms	system=saturn image_size=320x320 variant=identity phase=download_start bytes_done=0 bytes_total=100 percent=0 pack_index=3 pack_count=3 download_mbps= detail=
startup_timing	screenshot_media_ui_visibility	21ms	system=saturn row_seen=1 row_index=2 rendered=0 catalog_scan_visible=0 active_rows=3 visible_count=3 visible_systems=arcade,neogeo,saturn phase=download percent=0 summary_active=1 summary_done=2 summary_failed=0 summary_total=3
startup_timing	screenshot_media_progress	22ms	system=saturn image_size=320x320 variant=identity phase=done bytes_done=100 bytes_total=100 percent=100 pack_index=3 pack_count=3 download_mbps= detail=
startup_timing	screenshot_media_update_done	23ms	packs=3 current=0 missing=3 stale=0 downloaded=3 failed=0
EOF
  media_completion_report "$log" selftest >"$tmp/completion.tsv"
  grep -q $'media_completion_tsv\tlabel=selftest\tvalid=1' "$tmp/completion.tsv"
  sed '/screenshot_media_update_done/d' "$log" >"$tmp/missing-done.log"
  if media_completion_report "$tmp/missing-done.log" selftest >/dev/null 2>&1; then
    echo "media completion self-test accepted missing worker Done" >&2
    return 1
  fi
  sed '/system=saturn.*phase=done/d' "$log" >"$tmp/missing-terminal.log"
  if media_completion_report "$tmp/missing-terminal.log" selftest >/dev/null 2>&1; then
    echo "media completion self-test accepted a requested pack without a terminal" >&2
    return 1
  fi
  cp "$log" "$tmp/duplicate-terminal.log"
  sed -n '/system=saturn.*phase=done/p' "$log" >>"$tmp/duplicate-terminal.log"
  if media_completion_report "$tmp/duplicate-terminal.log" selftest >/dev/null 2>&1; then
    echo "media completion self-test accepted a duplicate pack terminal" >&2
    return 1
  fi
  summarize_media_log "$log" selftest abc123 "arcade,neogeo,saturn" >"$out"
  grep -q $'media_cold_boot_tsv\tlabel=selftest\tsystem=arcade\t.*progress_download_seen=1\tui_row_seen=1\tui_rendered_seen=1\tterminal=done' "$out"
  grep -q $'media_cold_boot_tsv\tlabel=selftest\tsystem=neogeo\t.*progress_download_seen=1\tui_row_seen=0\tui_rendered_seen=0\tterminal=done' "$out"
  grep -q $'media_cold_boot_tsv\tlabel=selftest\tsystem=saturn\t.*progress_download_seen=1\tui_row_seen=1\tui_rendered_seen=0\tterminal=done' "$out"
  cat >"$trace" <<'EOF'
frame	elapsed_us	cache_state	startup_elapsed_us	monotonic_us	main_present_backend	main_present_status
1	1000	exact	7000	1007000	fpga-vblank-latch-hidden	ok
2	2000	exact	10000	1010000	fpga-vblank-latch-hidden	ok
3	3000	exact	15000	1015000	fpga-vblank-latch-hidden	ok
4	1500000	exact	1500000	2500000	fpga-vblank-latch-hidden	ok
EOF
  thread_trace="$tmp/thread.tsv"
  cat >"$thread_trace" <<'EOF'
thread_sample_tsv	sample	ts_unix	interval_start_monotonic_us	monotonic_us	pid	tid	thread_name	state	processor	utime_jiffies	stime_jiffies	utime_delta_jiffies	stime_delta_jiffies	voluntary_ctxt_switches	nonvoluntary_ctxt_switches	voluntary_delta	nonvoluntary_delta	vmrss_kb	vmhwm_kb	sched_exec_runtime_ms	sched_nr_switches	sched_wait_sum_ms
thread_sample_tsv	1	1	1005000	1016000	10	10	mister-magik-fb	R	1	2	1	1	0	0	0	0	0	1000	1000	0	0	0
thread_sample_tsv	1	1	1005000	1016000	10	11	screenshot-medi	S	0	2	1	1	0	0	0	0	0	1000	1000	0	0	0
thread_sample_tsv	1	1	1005000	1016000	10	12	preview-selecte	S	0	2	1	1	0	0	0	0	0	1000	1000	0	0	0
EOF
  arcade_trace_complete "$trace" 2
  head -3 "$trace" >"$tmp/truncated.tsv"
  if arcade_trace_complete "$tmp/truncated.tsv" 2; then
    echo "Arcade trace completion self-test accepted a truncated trace" >&2
    return 1
  fi
  media_arcade_contention_report "$log" "$trace" "$thread_trace" selftest "$tmp/subset.tsv" 2 1 1 1 >"$tmp/contention.tsv"
  grep -q $'media_arcade_overlap_tsv\tlabel=selftest\tvalid=1' "$tmp/contention.tsv"
  grep -q $'media_arcade_preview_evidence_tsv\tlabel=selftest\tvalid=1' "$tmp/contention.tsv"
  grep -q $'media_arcade_thread_gate_tsv\tlabel=selftest\tvalid=1' "$tmp/contention.tsv"
  [[ "$(wc -l <"$tmp/subset.tsv" | tr -d ' ')" -eq 4 ]]
  awk -F '\t' 'NR == 1 { print; next } { $4 = $2; print }' OFS='\t' "$trace" >"$tmp/wrong-clock.tsv"
  if media_arcade_contention_report "$log" "$tmp/wrong-clock.tsv" "$thread_trace" selftest "$tmp/subset-wrong.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted rebased elapsed_us as startup time" >&2
    return 1
  fi
  sed '/preview-selecte/d' "$thread_trace" >"$tmp/missing-thread.tsv"
  if media_arcade_contention_report "$log" "$trace" "$tmp/missing-thread.tsv" selftest "$tmp/subset-missing-thread.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted missing selected-preview thread evidence" >&2
    return 1
  fi
  awk -F '\t' 'BEGIN { OFS=FS } NR == 1 { for (i=1;i<=NF;i++) col[$i]=i; print; next } { $(col["utime_delta_jiffies"])=0; $(col["stime_delta_jiffies"])=0; print }' \
    "$thread_trace" >"$tmp/inactive-thread.tsv"
  if media_arcade_contention_report "$log" "$trace" "$tmp/inactive-thread.tsv" selftest "$tmp/subset-inactive-thread.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted idle thread observations" >&2
    return 1
  fi
  awk -F '\t' 'BEGIN { OFS=FS } NR == 1 { for (i=1;i<=NF;i++) col[$i]=i; print; next } { $(col["interval_start_monotonic_us"])=3000000; $(col["monotonic_us"])=4000000; print }' \
    "$thread_trace" >"$tmp/unaligned-thread.tsv"
  if media_arcade_contention_report "$log" "$trace" "$tmp/unaligned-thread.tsv" selftest "$tmp/subset-unaligned-thread.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted temporally unaligned thread observations" >&2
    return 1
  fi
  sed '/preview_selected_applied/d' "$log" >"$tmp/missing-preview.log"
  if media_arcade_contention_report "$tmp/missing-preview.log" "$trace" "$thread_trace" selftest "$tmp/subset-missing-preview.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted missing selected-preview latency evidence" >&2
    return 1
  fi
  sed 's/age_us=50000/age_us=300000/' "$log" >"$tmp/slow-preview.log"
  if media_arcade_contention_report "$tmp/slow-preview.log" "$trace" "$thread_trace" selftest "$tmp/subset-slow.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted selected-preview p99 above 250ms" >&2
    return 1
  fi
  sed 's/\texact\t/\tstale\t/' "$trace" >"$tmp/stale-preview.tsv"
  if media_arcade_contention_report "$log" "$tmp/stale-preview.tsv" "$thread_trace" selftest "$tmp/subset-stale.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted stale cache-state frames" >&2
    return 1
  fi
  sed '2s/fpga-vblank-latch-hidden/fb0-dirty/' "$trace" >"$tmp/unstable-backend.tsv"
  if media_arcade_contention_report "$log" "$tmp/unstable-backend.tsv" "$thread_trace" selftest "$tmp/subset-backend.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted an unstable presentation backend" >&2
    return 1
  fi
  sed '2s/\tok$/\ttimeout/' "$trace" >"$tmp/unstable-status.tsv"
  if media_arcade_contention_report "$log" "$tmp/unstable-status.tsv" "$thread_trace" selftest "$tmp/subset-status.tsv" 2 1 1 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted a non-ok presentation status" >&2
    return 1
  fi
  cat >"$tmp/gap.log" <<'EOF'
startup_timing	screenshot_media_progress	0ms	system=arcade phase=download_start pack_index=1
startup_timing	screenshot_media_progress	10ms	system=arcade phase=done pack_index=1
startup_timing	preview_selected_applied	20ms	system=arcade load_source=index_pread age_us=1000
startup_timing	screenshot_media_progress	30ms	system=neogeo phase=download_start pack_index=2
startup_timing	screenshot_media_progress	40ms	system=neogeo phase=done pack_index=2
EOF
  cat >"$tmp/gap.tsv" <<'EOF'
frame	elapsed_us	cache_state	startup_elapsed_us	monotonic_us	main_present_backend	main_present_status
1	1000	exact	20000	1020000	fpga-vblank-latch-hidden	ok
2	1500000	exact	20000	1020000	fpga-vblank-latch-hidden	ok
EOF
  if media_arcade_contention_report "$tmp/gap.log" "$tmp/gap.tsv" "$thread_trace" selftest "$tmp/subset-gap.tsv" 1 1 0 1 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted a frame only inside the coarse media hull" >&2
    return 1
  fi
  if media_arcade_contention_report "$log" "$trace" "$thread_trace" selftest "$tmp/subset-material.tsv" 300 180 60 10 >/dev/null 2>&1; then
    echo "media/Arcade contention self-test accepted insufficient production overlap/sample coverage" >&2
    return 1
  fi
  rm -rf "$tmp"
  echo "profile-media-cold-boot self-test ok"
}

if [[ "$self_test" -eq 1 ]]; then
  run_self_test
  exit 0
fi

case "$deploy" in
  device)
    deploy_args=(--device --ui-scope launcher)
    if [[ "$arcade_trace_secs" -gt 0 ]]; then deploy_args+=(--bench-tools); fi
    "$HERE/scripts/deploy-rust.sh" "${deploy_args[@]}"
    ;;
  skip) : ;;
  *) echo "internal deploy mode error: $deploy" >&2; exit 2 ;;
esac

binary_path="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
deployed_sha256="$(bench_context_remote_sha256 "$MISTER" "$REMOTE_BIN" || true)"
deployed_sha256="${deployed_sha256:-missing}"
expected_features="ui"
if [[ "$arcade_trace_secs" -gt 0 ]]; then expected_features="ui,bench-tools"; fi
if ! bench_context_require_binary_contract "$binary_path" "$deployed_sha256" "$expected_features" release-device launcher; then
  local_sha256="$(bench_context_sha256_file "$binary_path")"
  built_features="$(bench_context_binary_features "$binary_path")"
  echo "media cold boot binary contract verification failed local=$local_sha256 deployed=$deployed_sha256 built_features=$built_features expected_features=$expected_features" >&2
  exit 1
fi

if [[ "$replace_label" -eq 1 && -f "$TSV" ]]; then
  tmp_tsv="$(mktemp)"
  grep -v $'\tlabel='"$label" "$TSV" >"$tmp_tsv" || true
  mv "$tmp_tsv" "$TSV"
fi

commit="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
local_log="$OUT_DIR/${label}.log"
local_status="$OUT_DIR/${label}.status.txt"
local_status_json="$OUT_DIR/${label}.status.json"
remote_arcade_trace="/tmp/${label}-media-arcade.tsv"
local_arcade_trace="$OUT_DIR/${label}.arcade.tsv"
contention_report="$OUT_DIR/${label}.media-arcade-contention.tsv"
completion_report="$OUT_DIR/${label}.media-completion.tsv"
contention_subset_trace="$OUT_DIR/${label}.media-arcade-overlap.tsv"
frame_pacing_report="$OUT_DIR/${label}.arcade-frame-pacing.tsv"
latch_drop_report="$OUT_DIR/${label}.arcade-latch-drops.tsv"
local_latch_before="$OUT_DIR/${label}.fpga-latch-before.log"
local_latch_after="$OUT_DIR/${label}.fpga-latch-after.log"
snapshot_dir="$OUT_DIR/${label}-snapshot"
report="$OUT_DIR/${label}.report.tsv"
cleanup_report="$OUT_DIR/${label}.cleanup.txt"
cleanup_row="$OUT_DIR/${label}.cleanup.tsv"
env_file="$(mktemp)"
profile_media_cold_boot_cleanup_done=0
profile_media_cold_boot_cleanup_status=0
profile_media_cold_boot_pending_signal=""

profile_media_cold_boot_cleanup() {
  local cleanup_status=0 arming_status=0
  if [[ "$profile_media_cold_boot_cleanup_done" -eq 1 ]]; then
    return "$profile_media_cold_boot_cleanup_status"
  fi
  profile_media_cold_boot_cleanup_done=1
  thread_sample_stop || cleanup_status=1
  rm -f "$env_file"
  benchmark_cleanup_clear_launcher_env "$MISTER" 30 >/dev/null 2>&1 || cleanup_status=1
  if [[ "$cleanup_assets" -eq 1 ]]; then
    "$MISTER" run "rm -rf $(shell_quote "$asset_dir")" >/dev/null 2>&1 || cleanup_status=1
  fi
  "$MISTER" run "rm -f $(shell_quote "$remote_arcade_trace")" >/dev/null 2>&1 || cleanup_status=1
  benchmark_cleanup_assert_no_arming_files "$MISTER" "$cleanup_report" || arming_status=1
  if [[ "$arming_status" -eq 0 && "$cleanup_status" -eq 0 ]]; then
    printf 'cleanup_tsv\tlabel=%s\tvalid=1\tinvalid_reason=ok\n' "$label" >"$cleanup_row"
  else
    printf 'cleanup_tsv\tlabel=%s\tvalid=0\tinvalid_reason=cleanup-command-or-stale-arming\n' "$label" >"$cleanup_row"
    cleanup_status=1
  fi
  profile_media_cold_boot_cleanup_status="$cleanup_status"
  return "$cleanup_status"
}
benchmark_cleanup_install profile_media_cold_boot_cleanup

{
  printf 'export MISTER_CATALOG_REFRESH=default\n'
  if [[ "$arcade_trace_secs" -gt 0 ]]; then
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_START_SYSTEM=arcade\n'
    printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$arcade_scenario"
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$arcade_trace_secs"
    printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$remote_arcade_trace"
    printf 'export MISTER_PREVIEW_TRACE=1\n'
    printf 'export MISTER_MEDIA_BENCH_CONTENTION=1\n'
  else
    printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
  fi
  printf 'export MISTER_MEDIA_UPDATE=download\n'
  printf 'export MISTER_MEDIA_CONCURRENCY=1\n'
  printf 'export MISTER_MEDIA_ASSET_DIR=%q\n' "$asset_dir"
  printf 'export MISTER_MEDIA_SIZE=%q\n' "$image_size"
  printf 'export MISTER_MEDIA_MANIFEST_URL=%q\n' "$manifest_url"
} >"$env_file"

echo "==> media cold boot label=$label commit=$commit asset_dir=$asset_dir reset_catalog=$reset_catalog timeout=${timeout_secs}s"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
reset_cmd="rm -rf $(shell_quote "$asset_dir"); rm -f $(shell_quote "$REMOTE_LOG") $(shell_quote "$remote_arcade_trace")"
if [[ "$reset_catalog" -eq 1 ]]; then
  reset_cmd+=" $(shell_quote "$REMOTE_DB") $(shell_quote "$REMOTE_SUMMARY")"
fi
reset_cmd+="; sync"
"$MISTER" run "$reset_cmd" >/dev/null
"$MISTER" reboot-wait
if [[ "$arcade_trace_secs" -gt 0 ]]; then
  "$MISTER" run "'$REMOTE_BIN' fpga-latch-report" >"$local_latch_before"
fi
thread_sample_start "$label" "media-cold-boot" "$OUT_DIR" "$timeout_secs"

deadline=$((SECONDS + timeout_secs))
snapshot_taken=0
run_done=0
run_failed=0
arcade_done=0
if [[ "$arcade_trace_secs" -eq 0 ]]; then arcade_done=1; fi
completion_reason="incomplete"
capture_framebuffer_snapshot() {
  rm -rf "$snapshot_dir"
  mkdir -p "$snapshot_dir"
  "$MISTER" status --json >"$snapshot_dir/status.json" 2>/dev/null || true
  "$MISTER" agent framebuffer-capture "$snapshot_dir/fb0.png" --json "$snapshot_dir/framebuffer.json" >/dev/null 2>&1
}
while (( SECONDS < deadline )); do
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  if [[ "$arcade_trace_secs" -gt 0 ]]; then
    "$MISTER" get "$remote_arcade_trace" "$local_arcade_trace" >/dev/null 2>&1 || true
    if [[ -s "$local_arcade_trace" ]] && arcade_trace_complete "$local_arcade_trace" "$arcade_trace_secs"; then
      arcade_done=1
    fi
  fi
  if [[ "$snapshot_taken" -eq 0 ]] && grep -q $'^startup_timing\tscreenshot_media_progress\t.*phase=download' "$local_log" 2>/dev/null; then
    if capture_framebuffer_snapshot; then
      snapshot_taken=1
    else
      snapshot_taken=2
    fi
  fi
  if grep -q $'^startup_timing\tscreenshot_media_update_done\t' "$local_log" 2>/dev/null; then
    run_done=1
    completion_reason="worker_done"
  fi
  if grep -q $'^startup_timing\tscreenshot_media_update_failed\t' "$local_log" 2>/dev/null; then
    run_failed=1
    completion_reason="worker_failed"
    break
  fi
  if [[ "$run_done" -eq 1 && "$arcade_done" -eq 1 ]]; then
    break
  fi
  sleep 3
done

"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
"$MISTER" status >"$local_status" 2>&1 || true
"$MISTER" status --json >"$local_status_json" 2>/dev/null || true
if [[ "$arcade_trace_secs" -gt 0 ]]; then
  "$MISTER" get "$remote_arcade_trace" "$local_arcade_trace" >/dev/null 2>&1 || true
  "$MISTER" run "'$REMOTE_BIN' fpga-latch-report" >"$local_latch_after" 2>/dev/null || true
fi
thread_sample_stop
thread_sample_collect
if [[ "$snapshot_taken" -eq 0 ]]; then
  capture_framebuffer_snapshot || snapshot_taken=2
fi

contention_status=0
completion_status=0
frame_pacing_status=0
latch_drop_status=0
correctness_contention_status=0
set +e
media_completion_report "$local_log" "$label" >"$completion_report"
completion_status=$?
set -e
if [[ "$arcade_trace_secs" -gt 0 ]]; then
  set +e
  media_arcade_contention_report \
    "$local_log" "$local_arcade_trace" "$thread_sample_local_tsv" "$label" "$contention_subset_trace" \
    "$contention_min_overlap_frames" "$contention_min_download_frames" \
    "$contention_min_publish_frames" "$contention_min_selected_applies" >"$contention_report"
  contention_status=$?
  "$HERE/scripts/checks/check-frame-pacing-trace.py" "$label-media-arcade-overlap" "$contention_subset_trace" 14500 16000 16667 "$arcade_scenario" vsync-integrity >"$frame_pacing_report"
  frame_pacing_status=$?
  "$HERE/scripts/bench/analyze/analyze-max-scroll-drops.py" "$local_arcade_trace" --label "$label-media-arcade" \
    --status-json "$local_status_json" --ignore-elapsed-zero --expect-backend fpga-vblank-latch-hidden \
    --fpga-latch-report-before "$local_latch_before" --fpga-latch-report-after "$local_latch_after" >"$latch_drop_report"
  latch_drop_status=$?
  set -e
  if [[ "$contention_correctness_only" -eq 1 ]]; then
    if ! grep -q $'^media_arcade_overlap_tsv\t.*\tvalid=1\tinvalid_reason=ok' "$contention_report" ||
       ! grep -q $'^media_arcade_presentation_tsv\t.*\tvalid=1\tinvalid_reason=ok' "$contention_report"; then
      correctness_contention_status=9
    fi
  fi
fi

cleanup_status=0
set +e
trap 'profile_media_cold_boot_pending_signal=INT' INT
trap 'profile_media_cold_boot_pending_signal=TERM' TERM
profile_media_cold_boot_cleanup 0 NORMAL
cleanup_status=$?
set -e
trap - EXIT INT TERM
benchmark_cleanup_callback=""
interrupted_status=0
case "$profile_media_cold_boot_pending_signal" in
  INT) interrupted_status=130; cleanup_status=1 ;;
  TERM) interrupted_status=143; cleanup_status=1 ;;
esac
if [[ "$interrupted_status" -ne 0 ]]; then
  printf 'cleanup_tsv\tlabel=%s\tvalid=0\tinvalid_reason=interrupted-during-cleanup\tsignal=%s\n' \
    "$label" "$profile_media_cold_boot_pending_signal" >"$cleanup_row"
fi

{
  emit_run_context_row
  emit_contention_contract_row
  emit_artifact_row "log" "$local_log" "$REMOTE_LOG"
  emit_artifact_row "status" "$local_status" "scripts/mister status"
  emit_artifact_row "status-json" "$local_status_json" "scripts/mister status --json"
  emit_artifact_row "snapshot-status" "$snapshot_dir/status.json" "scripts/mister status --json"
  emit_artifact_row "snapshot-png" "$snapshot_dir/fb0.png" "agent framebuffer_capture"
  emit_artifact_row "media-completion" "$completion_report"
  cat "$completion_report"
  if [[ "$arcade_trace_secs" -gt 0 ]]; then
    emit_artifact_row "media-arcade-trace" "$local_arcade_trace" "$remote_arcade_trace"
    emit_artifact_row "media-arcade-contention" "$contention_report"
    emit_artifact_row "media-arcade-overlap-trace" "$contention_subset_trace"
    emit_artifact_row "media-arcade-frame-pacing" "$frame_pacing_report"
    emit_artifact_row "media-arcade-latch-drops" "$latch_drop_report"
    emit_artifact_row "media-arcade-fpga-latch-before" "$local_latch_before" "fpga-latch-report"
    emit_artifact_row "media-arcade-fpga-latch-after" "$local_latch_after" "fpga-latch-report"
    cat "$contention_report" "$frame_pacing_report" "$latch_drop_report"
  fi
  thread_sample_emit_artifacts
  if [[ "$arcade_trace_secs" -gt 0 ]]; then
    thread_sample_emit_summary "$label" "media-arcade-contention"
  else
    thread_sample_emit_summary "$label" "media-cold-boot"
  fi
  summarize_media_log "$local_log" "$label" "$commit" "$systems_csv"
  cat "$cleanup_row"
  if [[ "$run_done" -eq 1 && "$arcade_done" -eq 1 && "$completion_status" -eq 0 && "$cleanup_status" -eq 0 &&
        ( "$contention_correctness_only" -eq 1 && "$correctness_contention_status" -eq 0 ||
          "$contention_correctness_only" -eq 0 && "$contention_status" -eq 0 && "$frame_pacing_status" -eq 0 && "$latch_drop_status" -eq 0 ) ]]; then
    emit_validity_row "1" "ok" "completion=$completion_reason log=$local_log status=$local_status snapshot=$snapshot_dir asset_dir=$asset_dir reset_catalog=$reset_catalog"
  elif [[ "$cleanup_status" -ne 0 ]]; then
    if [[ "$interrupted_status" -ne 0 ]]; then
      emit_validity_row "0" "interrupted_during_cleanup" "signal=$profile_media_cold_boot_pending_signal completion=$completion_reason cleanup_report=$cleanup_report"
    else
      emit_validity_row "0" "cleanup_failed" "completion=$completion_reason cleanup_report=$cleanup_report"
    fi
  elif [[ "$run_failed" -eq 1 ]]; then
    emit_validity_row "0" "media_worker_failed" "completion=$completion_reason log=$local_log status=$local_status snapshot=$snapshot_dir asset_dir=$asset_dir reset_catalog=$reset_catalog"
  elif [[ "$run_done" -eq 1 && "$arcade_done" -eq 1 ]]; then
    emit_validity_row "0" "contention_gate_failed" "completion_status=$completion_status contention_status=$contention_status correctness_contention_status=$correctness_contention_status frame_pacing_status=$frame_pacing_status latch_drop_status=$latch_drop_status correctness_only=$contention_correctness_only"
  else
    emit_validity_row "0" "timeout" "completion=$completion_reason timeout_secs=$timeout_secs log=$local_log status=$local_status snapshot=$snapshot_dir asset_dir=$asset_dir reset_catalog=$reset_catalog"
  fi
} | tee "$report"

cat "$report" >>"$TSV"
echo "appended to $TSV"

if [[ "$interrupted_status" -ne 0 ]]; then
  exit "$interrupted_status"
fi
if [[ "$run_done" -ne 1 || "$arcade_done" -ne 1 || "$completion_status" -ne 0 || "$cleanup_status" -ne 0 ||
      ( "$contention_correctness_only" -eq 1 && "$correctness_contention_status" -ne 0 ) ||
      ( "$contention_correctness_only" -eq 0 && ( "$contention_status" -ne 0 || "$frame_pacing_status" -ne 0 || "$latch_drop_status" -ne 0 ) ) ]]; then
  echo "media cold boot/Arcade contention run did not complete or pass; latest log follows" >&2
  tail -100 "$local_log" >&2 || true
  exit 1
fi
