#!/usr/bin/env bash
# Reproduce and summarize first-boot screenshot media UI visibility.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/media-cold-boot"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-media-cold-boot.tsv"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
REMOTE_DB="/media/fat/mister-magik/library.sqlite3"
REMOTE_SUMMARY="/media/fat/mister-magik/library.summary.json"
DEFAULT_MANIFEST_URL="https://assets.mistermagik.com/mister-magik/v1/manifest.json"
ORIGINAL_ARGS=("$@")

label=""
deploy="skip"
replace_label=0
timeout_secs=900
manifest_url="${MISTER_MEDIA_MANIFEST_URL:-$DEFAULT_MANIFEST_URL}"
image_size="${MISTER_MEDIA_SIZE:-320x320}"
asset_dir=""
cleanup_assets=1
reset_catalog=1
self_test=0
systems_csv="arcade,neogeo,saturn"

usage() {
  cat <<'EOF'
Usage: scripts/profile-media-cold-boot.sh LABEL [--deploy-device|--skip-build] [--replace-label] [--timeout SECS] [--asset-dir PATH] [--keep-assets] [--keep-catalog] [--manifest-url URL] [--image-size SIZE] [--self-test]

Runs the supervised launcher after a reboot with a cold catalog and screenshot
media asset directory, then emits AI-readable rows showing whether arcade,
neogeo, and saturn were discovered, ensured, queued, downloaded, and visible in
the media progress UI model.

The default asset directory is a label-scoped temporary path under
/media/fat/mister-magik and is removed after the run. Use --keep-assets to
preserve it for inspection. Use --keep-catalog to reuse the installed catalog
database instead of forcing the first-boot scan path.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --deploy-device) deploy="device"; shift ;;
    --skip-build) deploy="skip"; shift ;;
    --replace-label) replace_label=1; shift ;;
    --timeout) timeout_secs="${2:?}"; shift 2 ;;
    --asset-dir) asset_dir="${2:?}"; cleanup_assets=0; shift 2 ;;
    --keep-assets) cleanup_assets=0; shift ;;
    --keep-catalog) reset_catalog=0; shift ;;
    --manifest-url) manifest_url="${2:?}"; shift 2 ;;
    --image-size) image_size="${2:?}"; shift 2 ;;
    --systems) systems_csv="${2:?}"; shift 2 ;;
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
if [[ -z "$asset_dir" ]]; then
  asset_dir="/media/fat/mister-magik/media-cold-boot-${label}-assets"
fi

mkdir -p "$OUT_DIR" "$BENCH_DIR"

tsv_value() {
  printf '%s' "$1" | tr '\t\r\n' '   '
}

shell_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
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

emit_run_context_row() {
  local commit command_text started_at
  commit="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  command_text="scripts/profile-media-cold-boot.sh ${ORIGINAL_ARGS[*]}"
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'run_context_tsv\tlabel=%s\tcommit=%s\tcommand=%s\tdevice=mister\tsystems=%s\tasset_dir=%s\timage_size=%s\tdeploy=%s\treset_catalog=%s\ttimeout_secs=%s\tstarted_at=%s\n' \
    "$(tsv_value "$label")" "$commit" "$(tsv_value "$command_text")" \
    "$(tsv_value "$systems_csv")" "$(tsv_value "$asset_dir")" "$(tsv_value "$image_size")" \
    "$deploy" "$reset_catalog" "$timeout_secs" "$started_at"
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

media_targets_terminal() {
  local log_path="$1"
  awk -F '\t' -v systems_csv="$systems_csv" '
    BEGIN {
      count = split(systems_csv, list, ",")
      for (i = 1; i <= count; i++) {
        wanted[list[i]] = 1
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
    $1 == "startup_timing" && $2 == "screenshot_media_progress" {
      detail = $4
      for (i = 5; i <= NF; i++) {
        detail = detail " " $i
      }
      sys = kv(detail, "system")
      phase = kv(detail, "phase")
      if ((sys in wanted) && (phase == "done" || phase == "failed" || phase == "skipped-current" || phase == "check-only")) {
        terminal[sys] = phase
      }
    }
    END {
      for (sys in wanted) {
        if (!(sys in terminal)) {
          exit 1
        }
      }
      exit 0
    }
  ' "$log_path"
}

run_self_test() {
  local tmp log out
  tmp="$(mktemp -d)"
  log="$tmp/media.log"
  out="$tmp/out.tsv"
  cat >"$log" <<'EOF'
startup_timing	catalog_system_discovered	1ms	system=arcade
startup_timing	screenshot_media_catalog_system_present	2ms	system=arcade source=catalog-seed
startup_timing	screenshot_media_catalog_ensure	3ms	system=arcade
startup_timing	screenshot_media_system_queued	4ms	system=arcade pack_index=1 requested=1 pending=1
startup_timing	screenshot_media_system_start	5ms	system=arcade pack_index=1 pack_count=3 pending=2 active=1 max_concurrent=1 policy=download
startup_timing	screenshot_media_progress	6ms	system=arcade image_size=320x320 variant=identity phase=download_start bytes_done=0 bytes_total=100 percent=0 pack_index=1 pack_count=3 download_mbps= detail=
startup_timing	screenshot_media_ui_visibility	7ms	system=arcade row_seen=1 row_index=0 rendered=1 catalog_scan_visible=1 active_rows=1 visible_count=1 visible_systems=arcade phase=download percent=0 summary_active=1 summary_done=0 summary_failed=0 summary_total=3
startup_timing	screenshot_media_progress	8ms	system=arcade image_size=320x320 variant=identity phase=done bytes_done=100 bytes_total=100 percent=100 pack_index=1 pack_count=3 download_mbps= detail=
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
  media_targets_terminal "$log"
  summarize_media_log "$log" selftest abc123 "arcade,neogeo,saturn" >"$out"
  grep -q $'media_cold_boot_tsv\tlabel=selftest\tsystem=arcade\t.*progress_download_seen=1\tui_row_seen=1\tui_rendered_seen=1\tterminal=done' "$out"
  grep -q $'media_cold_boot_tsv\tlabel=selftest\tsystem=neogeo\t.*progress_download_seen=1\tui_row_seen=0\tui_rendered_seen=0\tterminal=done' "$out"
  grep -q $'media_cold_boot_tsv\tlabel=selftest\tsystem=saturn\t.*progress_download_seen=1\tui_row_seen=1\tui_rendered_seen=0\tterminal=done' "$out"
  rm -rf "$tmp"
  echo "profile-media-cold-boot self-test ok"
}

if [[ "$self_test" -eq 1 ]]; then
  run_self_test
  exit 0
fi

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) : ;;
  *) echo "internal deploy mode error: $deploy" >&2; exit 2 ;;
esac

if [[ "$replace_label" -eq 1 && -f "$TSV" ]]; then
  tmp_tsv="$(mktemp)"
  grep -v $'\tlabel='"$label" "$TSV" >"$tmp_tsv" || true
  mv "$tmp_tsv" "$TSV"
fi

commit="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
local_log="$OUT_DIR/${label}.log"
local_status="$OUT_DIR/${label}.status.txt"
snapshot_dir="$OUT_DIR/${label}-snapshot"
report="$OUT_DIR/${label}.report.tsv"
env_file="$(mktemp)"

cleanup() {
  rm -f "$env_file"
  local cleanup_cmd
  cleanup_cmd="rm -f $(shell_quote "$REMOTE_ENV"); if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi"
  if [[ "$cleanup_assets" -eq 1 ]]; then
    cleanup_cmd+="; rm -rf $(shell_quote "$asset_dir")"
  fi
  "$MISTER" run "$cleanup_cmd" >/dev/null 2>&1 || true
}
trap cleanup EXIT

{
  printf 'export MISTER_CATALOG_REFRESH=default\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
  printf 'export MISTER_MEDIA_UPDATE=download\n'
  printf 'export MISTER_MEDIA_CONCURRENCY=1\n'
  printf 'export MISTER_MEDIA_ASSET_DIR=%q\n' "$asset_dir"
  printf 'export MISTER_MEDIA_SIZE=%q\n' "$image_size"
  printf 'export MISTER_MEDIA_MANIFEST_URL=%q\n' "$manifest_url"
} >"$env_file"

echo "==> media cold boot label=$label commit=$commit asset_dir=$asset_dir reset_catalog=$reset_catalog timeout=${timeout_secs}s"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
reset_cmd="rm -rf $(shell_quote "$asset_dir"); rm -f $(shell_quote "$REMOTE_LOG")"
if [[ "$reset_catalog" -eq 1 ]]; then
  reset_cmd+=" $(shell_quote "$REMOTE_DB") $(shell_quote "$REMOTE_SUMMARY")"
fi
reset_cmd+="; sync"
"$MISTER" run "$reset_cmd" >/dev/null
"$MISTER" reboot-wait

deadline=$((SECONDS + timeout_secs))
snapshot_taken=0
run_done=0
run_failed=0
completion_reason="incomplete"
while (( SECONDS < deadline )); do
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
  if [[ "$snapshot_taken" -eq 0 ]] && grep -q $'^startup_timing\tscreenshot_media_progress\t.*phase=download' "$local_log" 2>/dev/null; then
    rm -rf "$snapshot_dir"
    if "$MISTER" snapshot "$snapshot_dir" >/dev/null 2>&1; then
      snapshot_taken=1
    else
      snapshot_taken=2
    fi
  fi
  if grep -q $'^startup_timing\tscreenshot_media_update_done\t' "$local_log" 2>/dev/null; then
    run_done=1
    completion_reason="worker_done"
    break
  fi
  if grep -q $'^startup_timing\tscreenshot_media_update_failed\t' "$local_log" 2>/dev/null; then
    run_failed=1
    completion_reason="worker_failed"
    break
  fi
  if media_targets_terminal "$local_log" 2>/dev/null; then
    run_done=1
    completion_reason="targets_terminal"
    break
  fi
  sleep 3
done

"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null 2>&1 || true
"$MISTER" status >"$local_status" 2>&1 || true
if [[ "$snapshot_taken" -eq 0 ]]; then
  rm -rf "$snapshot_dir"
  "$MISTER" snapshot "$snapshot_dir" >/dev/null 2>&1 || snapshot_taken=2
fi

{
  emit_run_context_row
  emit_artifact_row "log" "$local_log" "$REMOTE_LOG"
  emit_artifact_row "status" "$local_status" "scripts/mister status"
  emit_artifact_row "snapshot-status" "$snapshot_dir/status.json" "scripts/mister snapshot"
  emit_artifact_row "snapshot-png" "$snapshot_dir/fb0.png" "scripts/mister snapshot"
  summarize_media_log "$local_log" "$label" "$commit" "$systems_csv"
  if [[ "$run_done" -eq 1 ]]; then
    emit_validity_row "1" "ok" "completion=$completion_reason log=$local_log status=$local_status snapshot=$snapshot_dir asset_dir=$asset_dir reset_catalog=$reset_catalog"
  elif [[ "$run_failed" -eq 1 ]]; then
    emit_validity_row "0" "media_worker_failed" "completion=$completion_reason log=$local_log status=$local_status snapshot=$snapshot_dir asset_dir=$asset_dir reset_catalog=$reset_catalog"
  else
    emit_validity_row "0" "timeout" "completion=$completion_reason timeout_secs=$timeout_secs log=$local_log status=$local_status snapshot=$snapshot_dir asset_dir=$asset_dir reset_catalog=$reset_catalog"
  fi
} | tee "$report"

cat "$report" >>"$TSV"
echo "appended to $TSV"

if [[ "$run_done" -ne 1 ]]; then
  echo "media cold boot did not complete; latest log follows" >&2
  tail -100 "$local_log" >&2 || true
  exit 1
fi
