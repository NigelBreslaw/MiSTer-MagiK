#!/usr/bin/env bash
# Run a real launcher Arcade preview-scroll benchmark through MiSTer_MagiK.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-scroll-profiles"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"

usage() {
  cat <<'EOF'
Usage: scripts/profile-preview-scroll.sh [SECS] [SCENARIO] [LABEL] [--skip-build|--deploy-device] [--cpu-profile] [--self-test] [--visual-captures N]

Scenarios: velocity-scroll | held-scroll | turbo-hold | preview-step-hold
Runs the real launcher Arcade screen under Main_MiSTer supervision by writing
/media/fat/mister-magik/launcher.env and sending mister_magik_restart_launcher.

--cpu-profile builds/deploys the profiling binary, runs the same supervised
Arcade scenario with MISTER_PPROF=1, exits after the trace window, and pulls a
non-empty CPU SVG artifact.
--visual-captures captures fixed Arcade indices from the real launcher screen.

Do not use row-step scenarios such as list-scroll/smooth-scroll for Arcade
performance benchmarking. They do not reproduce real velocity scrolling.
EOF
}

secs="30"
scenario="velocity-scroll"
label="preview-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
self_test="0"
visual_captures="4"
allow_hotpath_misses="${MISTER_ALLOW_PREVIEW_HOTPATH_MISSES:-0}"
cpu_profile="0"
cpu_profile_remote_svg=""
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --cpu-profile) cpu_profile="1"; shift ;;
    --self-test) self_test="1"; shift ;;
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -ge 1 ]]; then secs="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -ge 2 ]]; then scenario="${positionals[1]}"; fi
if [[ "${#positionals[@]}" -ge 3 ]]; then label="${positionals[2]}"; fi
if [[ "${#positionals[@]}" -gt 3 ]]; then usage >&2; exit 2; fi

case "$scenario" in
  velocity-scroll|held-scroll|turbo-hold|preview-step-hold) ;;
  list-scroll|smooth-scroll|selected-first|stress-scroll|cache-warm|preview|preview-changes|screenshot-stress|preview-stress)
    echo "row-step/jump scenario '$scenario' is not valid for preview benchmarking; use velocity-scroll, turbo-hold, or preview-step-hold" >&2
    exit 2
    ;;
  *) echo "unknown scenario: $scenario" >&2; usage >&2; exit 2 ;;
esac
remote_scenario="$scenario"
if [[ "$remote_scenario" == "velocity-scroll" ]]; then remote_scenario="held-scroll"; fi
if [[ ! "$secs" =~ ^[0-9]+$ ]]; then echo "secs must be an integer" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ ! "$visual_captures" =~ ^[0-9]+$ ]]; then echo "--visual-captures must be an integer" >&2; exit 2; fi

mkdir -p "$OUT_DIR"
env_file="$(mktemp)"

cleanup() {
  rm -f "$env_file"
  if [[ "$self_test" == "1" ]]; then return; fi
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher ;;
  skip) : ;;
esac

if [[ "$cpu_profile" == "1" && "$self_test" != "1" ]]; then
  profile_bin="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb"
  echo "==> Build profiling binary for supervised Arcade CPU profile"
  "$HERE/magik-gui/build-arm.sh" --profile --ui-scope launcher
  echo "==> Deploy profiling binary for supervised Arcade CPU profile"
  if ! "$MISTER" agent deploy-magik-bin "$profile_bin" /media/fat/mister-magik/mister-magik-fb >/dev/null; then
    echo "agent deploy failed for profiling binary; falling back to device deploy transaction" >&2
    "$MISTER" deploy-magik-bin "$profile_bin" /media/fat/mister-magik/mister-magik-fb >/dev/null
  fi
fi

write_launcher_env() {
  local scenario_value="$1"
  local trace_path="$2"
  local selected_index="${3:-}"
  {
    printf 'export MISTER_CATALOG_REFRESH=off\n'
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$scenario_value"
    printf 'export MISTER_PREVIEW_TRACE=1\n'
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
    if [[ -n "$trace_path" ]]; then printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$trace_path"; fi
    if [[ -n "$selected_index" ]]; then printf 'export MISTER_ARCADE_SELECTED_INDEX=%q\n' "$selected_index"; fi
    if [[ "$cpu_profile" == "1" ]]; then
      printf 'export MISTER_PPROF=1\n'
      printf 'export MISTER_PPROF_OUT=%q\n' "$cpu_profile_remote_svg"
      printf 'export MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE=1\n'
    fi
  } >"$env_file"
}

restart_supervised_launcher() {
  local trace_path="$1"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "rm -f '$REMOTE_LOG' '$trace_path'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
}

run_case() {
  local name="$1"
  local remote_tsv="/tmp/${label}-${name}.tsv"
  local local_tsv="$OUT_DIR/${label}-${name}.tsv"
  local local_log="$OUT_DIR/${label}-${name}.log"
  local local_cpu_svg="$OUT_DIR/${label}-${name}-cpu.svg"
  cpu_profile_remote_svg="/tmp/${label}-${name}-cpu.svg"

  echo "==> $name supervised launcher Arcade scenario=$scenario remote_scenario=$remote_scenario secs=$secs transition=fixed-fade cpu_profile=$cpu_profile"
  if [[ "$cpu_profile" == "1" ]]; then
    "$MISTER" run "rm -f '$cpu_profile_remote_svg'" >/dev/null
  fi
  write_launcher_env "$remote_scenario" "$remote_tsv"
  restart_supervised_launcher "$remote_tsv"
  sleep $((secs + 7))
  if ! "$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null; then
    "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
    echo "$name failed; see $local_log" >&2
    exit 1
  fi
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
  echo "wrote $local_tsv"
  echo "wrote $local_log"
  if [[ "$cpu_profile" == "1" ]]; then
    if ! "$MISTER" get "$cpu_profile_remote_svg" "$local_cpu_svg" >/dev/null || [[ ! -s "$local_cpu_svg" ]]; then
      echo "$name CPU profile failed or produced an empty SVG; see $local_log" >&2
      exit 9
    fi
    if ! grep -q 'cpu_profile:' "$local_log"; then
      echo "$name CPU profile log does not contain cpu_profile output; see $local_log" >&2
      exit 9
    fi
    echo "wrote $local_cpu_svg"
  fi
}

capture_visuals() {
  local count="$visual_captures"
  if [[ "$count" == "0" ]]; then return; fi
  local visual_dir="$OUT_DIR/${label}-visuals"
  mkdir -p "$visual_dir"
  local indices=(0 7 14 21 28 35 42 49)
  local i idx idx_pad snap_dir png_out
  for ((i = 0; i < count && i < ${#indices[@]}; i++)); do
    idx="${indices[$i]}"
    idx_pad="$(printf "%03d" "$idx")"
    snap_dir="$visual_dir/idx${idx_pad}.snapshot"
    png_out="$visual_dir/idx${idx_pad}.png"
    echo "==> visual selected_index=$idx"
    write_launcher_env "idle" "" "$idx"
    restart_supervised_launcher "/tmp/${label}-visual-${idx_pad}.tsv"
    sleep 8
    "$MISTER" snapshot "$snap_dir" >/dev/null
    cp "$snap_dir/fb0.png" "$png_out"
    "$MISTER" get "$REMOTE_LOG" "$visual_dir/idx${idx_pad}.log" >/dev/null || true
    echo "wrote $png_out"
  done
}

summarize_trace() {
  local name="$1" tsv="$2" log="$3"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      n++
      wall = $(col["wall_us"]) + 0
      walls[n] = wall
      sum += wall
      if (wall > 16667) slow16++
      if (wall > 20000) slow20++
      state = $(col["cache_state"])
      states[state]++
    }
    END {
      if (n == 0) { printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name; exit }
      for (i = 1; i <= n; i++) for (j = i + 1; j <= n; j++) if (walls[j] < walls[i]) { tmp = walls[i]; walls[i] = walls[j]; walls[j] = tmp }
      p95_i = int(n * 0.95); if (p95_i < 1) p95_i = 1; if (p95_i > n) p95_i = n
      printf "%s\t%d\t%.0f\t%d\t%d\t%d\t%d\t%d\t%d\t%d\n", name, n, sum / n, walls[p95_i], slow16, slow20, states["exact"] + 0, states["cached"] + 0, states["stale"] + 0, states["placeholder"] + 0
    }
  ' "$tsv" | while IFS=$'\t' read -r n frames avg p95 slow16 slow20 exact cached stale placeholder; do
    local rss
    rss="$(sed -n 's/^rss_hwm_kb=//p' "$log" | tail -1)"
    printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" "$n" "$frames" "$avg" "$p95" "$slow16" "$slow20" "$exact" "$cached" "$stale" "$placeholder" "${rss:-0}"
  done
}

summarize_frame_pacing() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      has_pacing = ("vsync_source" in col) && ("loop_delta_us" in col)
      next
    }
    NF && $(col["frame"]) + 0 > 30 {
      n++
      wall = $(col["wall_us"]) + 0
      work = $(col["prepare_us"]) + $(col["slint_render_us"]) + $(col["custom_draw_us"]) + $(col["fb_present_us"])
      vsync = $(col["vsync_us"]) + 0
      loop_delta = has_pacing ? ($(col["loop_delta_us"]) + 0) : 0
      source = has_pacing ? $(col["vsync_source"]) : "unknown"
      miss_streak = has_pacing ? ($(col["vsync_miss_streak"]) + 0) : 0

      walls[n] = wall
      works[n] = work
      vsyncs[n] = vsync
      loops[n] = loop_delta
      wall_sum += wall
      work_sum += work
      vsync_sum += vsync
      loop_sum += loop_delta
      if (wall > 16667) slow_wall++
      if (wall > 17000) slow_wall_17++
      if (work > 16667) work_over++
      if (vsync > 10000) high_vsync++
      if (wall > 16667 && work <= 5000 && vsync > 10000) low_work_high_vsync++
      if (wall > 16667 && work > 12000) cpu_heavy_slow++
      sources[source]++
      if (miss_streak > max_miss_streak) max_miss_streak = miss_streak
    }
    END {
      if (n == 0) {
        printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name
        exit
      }
      for (i = 1; i <= n; i++) {
        for (j = i + 1; j <= n; j++) {
          if (walls[j] < walls[i]) { tmp = walls[i]; walls[i] = walls[j]; walls[j] = tmp }
          if (works[j] < works[i]) { tmp = works[i]; works[i] = works[j]; works[j] = tmp }
          if (vsyncs[j] < vsyncs[i]) { tmp = vsyncs[i]; vsyncs[i] = vsyncs[j]; vsyncs[j] = tmp }
          if (loops[j] < loops[i]) { tmp = loops[i]; loops[i] = loops[j]; loops[j] = tmp }
        }
      }
      p95 = int(n * 0.95); if (p95 < 1) p95 = 1; if (p95 > n) p95 = n
      p99 = int(n * 0.99); if (p99 < 1) p99 = 1; if (p99 > n) p99 = n
      printf "%s\t%d\t%.0f\t%d\t%d\t%.0f\t%d\t%d\t%.0f\t%d\t%d\t%.0f\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\n",
        name, n,
        wall_sum / n, walls[p95], walls[p99],
        work_sum / n, works[p95], works[p99],
        vsync_sum / n, vsyncs[p95], vsyncs[p99],
        loop_sum / n, loops[p95], loops[p99],
        slow_wall + 0, slow_wall_17 + 0, work_over + 0, high_vsync + 0,
        low_work_high_vsync + 0, cpu_heavy_slow + 0,
        sources["vsync"] + 0, sources["fallback"] + 0,
        sources["timeout"] + 0, sources["error"] + 0,
        max_miss_streak + 0
    }
  ' "$tsv"
}

summarize_custom_draw_phases() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      has_phases = ("arcade_list_update_us" in col) && ("preview_blit_us" in col) && ("effect_label_us" in col)
      next
    }
    NF && has_phases && $(col["frame"]) + 0 > 30 {
      n++
      phase["custom_draw_us", n] = $(col["custom_draw_us"]) + 0
      phase["arcade_list_update_us", n] = $(col["arcade_list_update_us"]) + 0
      phase["preview_blit_us", n] = $(col["preview_blit_us"]) + 0
      phase["effect_label_us", n] = $(col["effect_label_us"]) + 0
      phase["cached_present_us", n] = $(col["cached_present_us"]) + 0
      phase["overlay_present_us", n] = $(col["overlay_present_us"]) + 0
      sum["custom_draw_us"] += phase["custom_draw_us", n]
      sum["arcade_list_update_us"] += phase["arcade_list_update_us", n]
      sum["preview_blit_us"] += phase["preview_blit_us", n]
      sum["effect_label_us"] += phase["effect_label_us", n]
      sum["cached_present_us"] += phase["cached_present_us", n]
      sum["overlay_present_us"] += phase["overlay_present_us", n]
    }
    END {
      if (!has_phases || n == 0) {
        printf "%s\tmissing\t0\t0\t0\t0\n", name
        exit
      }
      fields[1] = "custom_draw_us"
      fields[2] = "arcade_list_update_us"
      fields[3] = "preview_blit_us"
      fields[4] = "effect_label_us"
      fields[5] = "cached_present_us"
      fields[6] = "overlay_present_us"
      for (field_i = 1; field_i <= 6; field_i++) {
        field = fields[field_i]
        for (i = 1; i <= n; i++) sorted[i] = phase[field, i]
        for (i = 1; i <= n; i++) {
          for (j = i + 1; j <= n; j++) {
            if (sorted[j] < sorted[i]) {
              tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp
            }
          }
        }
        p95 = int(n * 0.95); if (p95 < 1) p95 = 1; if (p95 > n) p95 = n
        p99 = int(n * 0.99); if (p99 < 1) p99 = 1; if (p99 > n) p99 = n
        printf "%s\t%s\t%d\t%.0f\t%d\t%d\n", name, field, n, sum[field] / n, sorted[p95], sorted[p99]
        delete sorted
      }
    }
  ' "$tsv"
}

report_steady_wall_gate() {
  local name="$1" tsv="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF && $(col["frame"]) + 0 > 30 {
      n++
      wall = $(col["wall_us"]) + 0
      if (wall > 16667) {
        slow++
        if (slow <= 10) printf "%s steady miss frame=%s wall_us=%s\n", name, $(col["frame"]), wall > "/dev/stderr"
      }
    }
    END {
      printf "%s steady_wall_report frames_after_30=%d slow_gt_16667=%d allow=%s\n", name, n, slow + 0, allow
    }
  ' "$tsv"
}

check_steady_work_gate() {
  local name="$1" tsv="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF && $(col["frame"]) + 0 > 30 {
      n++
      work = $(col["prepare_us"]) + $(col["slint_render_us"]) + $(col["custom_draw_us"]) + $(col["fb_present_us"])
      if (work > 16667) {
        slow++
        if (slow <= 10) printf "%s steady work miss frame=%s work_us=%s wall_us=%s vsync_us=%s\n", name, $(col["frame"]), work, $(col["wall_us"]), $(col["vsync_us"]) > "/dev/stderr"
      }
    }
    END {
      allowed = (allow == "1" || allow == "true" || allow == "yes" || allow == "on")
      if (slow > 0 && !allowed) {
        printf "%s steady work gate failed: frames_after_30=%d work_gt_16667=%d\n", name, n, slow > "/dev/stderr"
        exit 8
      }
      printf "%s steady_work_gate frames_after_30=%d work_gt_16667=%d allow=%s\n", name, n, slow + 0, allow
    }
  ' "$tsv"
}

summarize_preview_timing() {
  local name="$1" log="$2"
  awk -v name="$name" '
    /preview_trace (decoded|apply) / {
      total = read = decode = 0
      cache_hit = "unknown"
      load_source = "unknown"
      selected = "unknown"
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "total_us") total = kv[2] + 0
        else if (kv[1] == "read_us") read = kv[2] + 0
        else if (kv[1] == "decode_us") decode = kv[2] + 0
        else if (kv[1] == "cache_hit") cache_hit = kv[2]
        else if (kv[1] == "load_source") load_source = kv[2]
        else if (kv[1] == "selected") selected = kv[2]
      }
      n++
      total_sum += total; read_sum += read; decode_sum += decode
      if (total > total_max) total_max = total
      if (read > read_max) read_max = read
      if (decode > decode_max) decode_max = decode
      if (cache_hit == "true" || cache_hit == "1") cache_hits++
      if (load_source == "archive_mem") archive_mem++
      else if (load_source == "decoded_cache") decoded_cache++
      else if (read > 0) unexpected_file_reads++
      if (read > 5000) slow_reads++
      if (selected == "true" && load_source != "archive_mem" && load_source != "decoded_cache" && read > 0) selected_file_reads++
    }
    END {
      if (n == 0) {
        printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name
      } else {
        printf "%s\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%d\t%d\t%d\t%d\t%d\t%d\n",
          name, n, total_sum / n, total_max, read_sum / n, read_max, decode_sum / n, decode_max,
          cache_hits + 0, unexpected_file_reads + 0, slow_reads + 0,
          archive_mem + 0, decoded_cache + 0,
          selected_file_reads + 0
      }
    }
  ' "$log"
}

check_preview_hotpath_cache_gate() {
  local name="$1" log="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    /preview_trace cache_failed / {
      failed++
      selected = "unknown"
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "selected") selected = kv[2]
      }
      if (selected == "true") {
        selected_failed++
        if (selected_failed <= 10) printf "%s selected preview cache_failed line=%s\n", name, $0 > "/dev/stderr"
      }
    }
    END {
      allowed = (allow == "1" || allow == "true" || allow == "yes" || allow == "on")
      if (selected_failed > 0 && !allowed) {
        printf "%s preview hot-path cache gate failed: selected_cache_failed=%d cache_failed=%d\n", name, selected_failed + 0, failed + 0 > "/dev/stderr"
        exit 5
      }
      printf "%s preview_hotpath_cache_gate cache_failed=%d selected_cache_failed=%d allow=%s\n", name, failed + 0, selected_failed + 0, allow
    }
  ' "$log"
}

check_preview_hotpath_io_gate() {
  local name="$1" log="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    /preview_trace (decoded|apply) / {
      read = 0
      load_source = "unknown"
      selected = "unknown"
      is_apply = ($0 ~ /preview_trace apply /)
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "read_us") read = kv[2] + 0
        else if (kv[1] == "load_source") load_source = kv[2]
        else if (kv[1] == "selected") selected = kv[2]
      }
      if (load_source == "archive_mem") archive_backed = 1
      if (load_source != "archive_mem" && load_source != "decoded_cache" && read > 0) {
        unexpected_file_reads++
        if (read > 5000) slow_reads++
      }
      if (is_apply && selected == "true" && load_source != "archive_mem" && load_source != "decoded_cache" && read > 0) {
        selected_file_reads++
        if (selected_file_reads <= 10) {
          printf "%s selected preview unexpected file read: source=%s read_us=%d line=%s\n", name, load_source, read, $0 > "/dev/stderr"
        }
      }
    }
    END {
      allowed = (allow == "1" || allow == "true" || allow == "yes" || allow == "on")
      if (archive_backed && !allowed && (unexpected_file_reads > 0 || slow_reads > 0 || selected_file_reads > 0)) {
        printf "%s preview hot-path io gate failed: archive_backed=1 unexpected_file_reads=%d slow_reads=%d selected_file_reads=%d\n", name, unexpected_file_reads + 0, slow_reads + 0, selected_file_reads + 0 > "/dev/stderr"
        exit 7
      }
      printf "%s preview_hotpath_io_gate archive_backed=%d unexpected_file_reads=%d slow_reads=%d selected_file_reads=%d allow=%s\n", name, archive_backed + 0, unexpected_file_reads + 0, slow_reads + 0, selected_file_reads + 0, allow
    }
  ' "$log"
}

check_preview_visibility_gate() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      n++
      state = $(col["cache_state"])
      states[state]++
    }
    END {
      exact = states["exact"] + 0
      if (exact == 0) {
        printf "%s preview visibility gate failed: exact=0 frames=%d\n", name, n > "/dev/stderr"
        exit 6
      }
      printf "%s preview_visibility_gate frames=%d exact=%d\n", name, n, exact
    }
  ' "$tsv"
}

summarize_trace_by_effect() {
  local tsv="$1"
  awk '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; has_effect = ("transition_effect" in col); next }
    NF && has_effect {
      effect = $(col["transition_effect"])
      idx = ++n[effect]
      wall = $(col["wall_us"]) + 0
      walls[effect, idx] = wall
      sum[effect] += wall
      if (wall > 16667) slow16[effect]++
      if (wall > 20000) slow20[effect]++
      if (!(effect in seen)) { seen[effect] = 1; order[++order_n] = effect }
    }
    END {
      if (!has_effect) exit
      for (oi = 1; oi <= order_n; oi++) {
        effect = order[oi]; count = n[effect]
        for (i = 1; i <= count; i++) sorted[i] = walls[effect, i]
        for (i = 1; i <= count; i++) for (j = i + 1; j <= count; j++) if (sorted[j] < sorted[i]) { tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp }
        p95_i = int(count * 0.95); if (p95_i < 1) p95_i = 1; if (p95_i > count) p95_i = count
        printf "%s\t%d\t%.0f\t%d\t%d\t%d\n", effect, count, sum[effect] / count, sorted[p95_i], slow16[effect] + 0, slow20[effect] + 0
        delete sorted
      }
    }
  ' "$tsv"
}

check_velocity_motion() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      n++
      vi = $(col["visual_index"]) + 0
      frac = vi - int(vi); if (frac < 0) frac = -frac
      if (frac > 0.001 && frac < 0.999) fractional++
      if (seen) { delta = vi - last; if (delta < 0) delta = -delta; if (delta > 0.001) moving++ }
      last = vi; seen = 1
    }
    END { printf "%s\t%d\t%d\t%d\n", name, n, fractional, moving; if (moving > 0 && fractional == 0) exit 3 }
  ' "$tsv"
}

run_self_test() {
  local tmp
  tmp="$(mktemp -d)"

  local no_exact="$tmp/no-exact.tsv"
  local exact="$tmp/exact.tsv"
  cat >"$no_exact" <<'EOF'
frame	cache_state
0	cached
1	placeholder
EOF
  cat >"$exact" <<'EOF'
frame	cache_state
0	cached
1	exact
EOF

  if check_preview_visibility_gate selftest "$no_exact" >/dev/null 2>&1; then
    echo "preview visibility self-test expected exact=0 failure" >&2
    rm -rf "$tmp"
    exit 1
  fi
  check_preview_visibility_gate selftest "$exact" >/dev/null

  local wall_wait_ok="$tmp/wall-wait-ok.tsv"
  local work_slow="$tmp/work-slow.tsv"
  cat >"$wall_wait_ok" <<'EOF'
frame	prepare_us	slint_render_us	custom_draw_us	vsync_us	fb_present_us	wall_us
31	100	100	100	16000	500	16800
EOF
  cat >"$work_slow" <<'EOF'
frame	prepare_us	slint_render_us	custom_draw_us	vsync_us	fb_present_us	wall_us
31	1000	1000	14000	500	1000	17500
EOF
  check_steady_work_gate selftest "$wall_wait_ok" >/dev/null
  if check_steady_work_gate selftest "$work_slow" >/dev/null 2>&1; then
    echo "steady work self-test expected work over budget failure" >&2
    rm -rf "$tmp"
    exit 1
  fi

  local unexpected_read="$tmp/unexpected-read.log"
  local archive_mem_ok="$tmp/archive-mem-ok.log"
  cat >"$unexpected_read" <<'EOF'
preview_trace apply generation=1 priority=Selected selected=true age_us=1 load_source=unknown format=raw-rgb565 filter=Hybrid source=320x224 output=320x224 total_us=1000 read_us=900 decode_us=100 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=a.png
preview_trace decoded generation=2 priority=Prefetch cache_hit=0 load_source=archive_mem format=raw-rgb565 filter=hybrid source=320x224 output=320x224 total_us=300 read_us=0 decode_us=300 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=b.png
EOF
  cat >"$archive_mem_ok" <<'EOF'
preview_trace decoded generation=1 priority=Selected cache_hit=0 load_source=archive_mem format=raw-rgb565 filter=hybrid source=320x224 output=320x224 total_us=300 read_us=0 decode_us=300 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=a.png
preview_trace apply generation=1 priority=Selected selected=true age_us=1 load_source=archive_mem format=raw-rgb565 filter=Hybrid source=320x224 output=320x224 total_us=300 read_us=0 decode_us=300 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=a.png
EOF
  if check_preview_hotpath_io_gate selftest "$unexpected_read" >/dev/null 2>&1; then
    echo "preview hot-path io self-test expected unexpected file read failure" >&2
    rm -rf "$tmp"
    exit 1
  fi
  check_preview_hotpath_io_gate selftest "$archive_mem_ok" >/dev/null

  rm -rf "$tmp"
  echo "profile-preview-scroll self-test ok"
}

if [[ "$self_test" == "1" ]]; then
  run_self_test
  exit 0
fi

run_case arcade
capture_visuals

arcade_tsv="$OUT_DIR/${label}-arcade.tsv"
arcade_log="$OUT_DIR/${label}-arcade.log"

echo
echo $'case\tframes\tavg_wall_us\tp95_wall_us\tslow_gt_16_7ms\tslow_gt_20ms\texact\tcached\tstale\tplaceholder\trss_hwm_kb'
summarize_trace arcade "$arcade_tsv" "$arcade_log"

echo
echo $'transition_effect\tframes\tavg_wall_us\tp95_wall_us\tslow_gt_16_7ms\tslow_gt_20ms'
summarize_trace_by_effect "$arcade_tsv"

echo
echo $'motion_check\tframes\tfractional_visual_index_frames\tmoving_frames'
check_velocity_motion arcade "$arcade_tsv"

echo
echo "preview trace counts:"
printf "arcade decoded=%s apply=%s\n" \
  "$(grep -c 'preview_trace decoded' "$arcade_log" 2>/dev/null || true)" \
  "$(grep -c 'preview_trace apply' "$arcade_log" 2>/dev/null || true)"

echo
echo $'preview_timing\trows\tavg_total_us\tmax_total_us\tavg_read_us\tmax_read_us\tavg_decode_us\tmax_decode_us\tcache_hits\tunexpected_file_reads\tslow_reads\tarchive_mem\tdecoded_cache\tselected_file_reads'
summarize_preview_timing arcade "$arcade_log"

echo
echo $'frame_pacing\tframes_after_30\tavg_wall_us\tp95_wall_us\tp99_wall_us\tavg_work_us\tp95_work_us\tp99_work_us\tavg_vsync_us\tp95_vsync_us\tp99_vsync_us\tavg_loop_delta_us\tp95_loop_delta_us\tp99_loop_delta_us\tslow_wall_gt_16_7ms\tslow_wall_gt_17ms\twork_gt_16_7ms\tvsync_gt_10ms\tlow_work_high_vsync_slow\tcpu_heavy_slow\tvsync_source_vsync\tvsync_source_fallback\tvsync_source_timeout\tvsync_source_error\tmax_vsync_miss_streak'
summarize_frame_pacing arcade "$arcade_tsv"

echo
echo $'custom_draw_phase\tphase\tframes_after_30\tavg_us\tp95_us\tp99_us'
summarize_custom_draw_phases arcade "$arcade_tsv"

echo
check_preview_hotpath_cache_gate arcade "$arcade_log"
check_preview_hotpath_io_gate arcade "$arcade_log"
check_preview_visibility_gate arcade "$arcade_tsv"
check_steady_work_gate arcade "$arcade_tsv"
report_steady_wall_gate arcade "$arcade_tsv"
