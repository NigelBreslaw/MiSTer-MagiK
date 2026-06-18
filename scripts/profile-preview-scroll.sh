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
Usage: scripts/profile-preview-scroll.sh [SECS] [SCENARIO] [LABEL] [--skip-build|--deploy-device] [--list-only] [--self-test] [--fb-format 565] [--transition EFFECT|mega] [--transition-segment-secs N] [--transition-ms N] [--visual-captures N] [--preview-visual-pct N] [--preview-resize-filter off|nearest|box|lanczos|hybrid] [--preview-resize-max 320x320] [--preview-format raw-rgb565] [--preview-archive PATH]

Scenarios: velocity-scroll | held-scroll | turbo-hold | preview-step-hold
Runs the real launcher Arcade screen under Main_MiSTer supervision by writing
/media/fat/mister-magik/launcher.env and sending mister_magik_restart_launcher.

--list-only disables screenshot loading and the launcher's catalog refresh so
list-renderer changes can be measured without preview/catalog noise.
--fb-format is kept for old command lines, but UI profiling supports only 565.
--transition selects raw-preview screenshot transitions. Default is fade; `cut`
disables animation; `mega` cycles all effects.
--visual-captures captures fixed Arcade indices from the real launcher screen.

Do not use row-step scenarios such as list-scroll/smooth-scroll for Arcade
performance benchmarking. They do not reproduce real velocity scrolling.
EOF
}

secs="30"
scenario="velocity-scroll"
label="preview-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
list_only="0"
self_test="0"
preview_visual_pct=""
preview_resize_filter=""
preview_resize_max=""
preview_format=""
preview_archive=""
fb_format="565"
transition=""
transition_segment_secs=""
transition_ms=""
visual_captures="4"
allow_hotpath_misses="${MISTER_ALLOW_PREVIEW_HOTPATH_MISSES:-0}"
allow_no_exact_preview="${MISTER_ALLOW_PREVIEW_NO_EXACT:-0}"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --list-only) list_only="1"; shift ;;
    --self-test) self_test="1"; shift ;;
    --fb-format) fb_format="${2:-}"; shift 2 ;;
    --preview-blitter) echo "--preview-blitter was removed; previews are always raw565 via the Rust blitter" >&2; exit 2 ;;
    --transition) transition="${2:-}"; shift 2 ;;
    --transition-segment-secs) transition_segment_secs="${2:-}"; shift 2 ;;
    --transition-ms) transition_ms="${2:-}"; shift 2 ;;
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    --preview-visual-pct) preview_visual_pct="${2:-}"; shift 2 ;;
    --preview-resize-filter) preview_resize_filter="${2:-}"; shift 2 ;;
    --preview-resize-max) preview_resize_max="${2:-}"; shift 2 ;;
    --preview-format) preview_format="${2:-}"; shift 2 ;;
    --preview-archive) preview_archive="${2:-}"; shift 2 ;;
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
if [[ -n "$preview_visual_pct" && ! "$preview_visual_pct" =~ ^[0-9]+$ ]]; then echo "--preview-visual-pct must be an integer" >&2; exit 2; fi
case "$preview_resize_filter" in ""|off|nearest|box|lanczos|hybrid) ;; *) echo "--preview-resize-filter must be off, nearest, box, lanczos, or hybrid" >&2; exit 2 ;; esac
if [[ -n "$preview_resize_max" && ! "$preview_resize_max" =~ ^[0-9]+[xX][0-9]+$ ]]; then echo "--preview-resize-max must look like 320x320" >&2; exit 2; fi
case "$preview_format" in ""|raw-rgb565|raw565|rgb565|565) ;; *) echo "--preview-format must be raw-rgb565" >&2; exit 2 ;; esac
if [[ -n "$preview_archive" && ! "$preview_archive" =~ ^[A-Za-z0-9_./:-]+$ ]]; then echo "--preview-archive contains unsupported characters" >&2; exit 2; fi
case "$fb_format" in 565) ;; *) echo "--fb-format must be 565; RGB888 UI support was removed" >&2; exit 2 ;; esac
if [[ -n "$transition" && ! "$transition" =~ ^[A-Za-z0-9_,.-]+$ ]]; then echo "--transition must be a comma-separated transition label list or mega" >&2; exit 2; fi
if [[ -n "$transition_segment_secs" && ! "$transition_segment_secs" =~ ^[0-9]+$ ]]; then echo "--transition-segment-secs must be an integer" >&2; exit 2; fi
if [[ -n "$transition_ms" && ! "$transition_ms" =~ ^[0-9]+$ ]]; then echo "--transition-ms must be an integer" >&2; exit 2; fi
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

write_launcher_env() {
  local scenario_value="$1"
  local trace_path="$2"
  local selected_index="${3:-}"
  {
    printf 'export MISTER_FB_FORMAT=%q\n' "$fb_format"
    printf 'export MISTER_CATALOG_REFRESH=off\n'
    printf 'export MISTER_MAGIK_LIBRARY_REFRESH_DELAY_SECS=9999\n'
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$scenario_value"
    printf 'export MISTER_PREVIEW_TRACE=1\n'
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
    if [[ -n "$trace_path" ]]; then printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$trace_path"; fi
    if [[ "$list_only" == "1" ]]; then printf 'export MISTER_PREVIEW_LOADING=off\n'; fi
    if [[ -n "$preview_visual_pct" ]]; then printf 'export MISTER_PREVIEW_VISUAL_PCT=%q\n' "$preview_visual_pct"; fi
    if [[ -n "$preview_resize_filter" ]]; then printf 'export MISTER_PREVIEW_RESIZE_FILTER=%q\n' "$preview_resize_filter"; fi
    if [[ -n "$preview_resize_max" ]]; then printf 'export MISTER_PREVIEW_RESIZE_MAX=%q\n' "$preview_resize_max"; fi
    if [[ -n "$preview_format" ]]; then printf 'export MISTER_PREVIEW_FORMAT=%q\n' "$preview_format"; fi
    if [[ -n "$preview_archive" ]]; then printf 'export MISTER_PREVIEW_ARCHIVE=%q\n' "$preview_archive"; fi
    if [[ -n "$transition" ]]; then printf 'export MISTER_PREVIEW_TRANSITION=%q\n' "$transition"; fi
    if [[ -n "$transition_segment_secs" ]]; then printf 'export MISTER_PREVIEW_TRANSITION_SEGMENT_SECS=%q\n' "$transition_segment_secs"; fi
    if [[ -n "$transition_ms" ]]; then printf 'export MISTER_PREVIEW_TRANSITION_MS=%q\n' "$transition_ms"; fi
    if [[ -n "$selected_index" ]]; then printf 'export MISTER_ARCADE_SELECTED_INDEX=%q\n' "$selected_index"; fi
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

  echo "==> $name supervised launcher Arcade scenario=$scenario remote_scenario=$remote_scenario secs=$secs fb_format=$fb_format transition=${transition:-fade} list_only=$list_only preview_visual_pct=${preview_visual_pct:-100} preview_resize_filter=${preview_resize_filter:-app-default} preview_resize_max=${preview_resize_max:-app-default} preview_format=${preview_format:-app-default} preview_archive=${preview_archive:-none}"
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
}

capture_visuals() {
  local count="$visual_captures"
  if [[ "$count" == "0" || "$list_only" == "1" ]]; then return; fi
  local visual_dir="$OUT_DIR/${label}-visuals"
  mkdir -p "$visual_dir"
  local indices=(0 7 14 21 28 35 42 49)
  local i idx idx_pad snap_dir png_out
  for ((i = 0; i < count && i < ${#indices[@]}; i++)); do
    idx="${indices[$i]}"
    idx_pad="$(printf "%03d" "$idx")"
    snap_dir="$visual_dir/${fb_format}-idx${idx_pad}.snapshot"
    png_out="$visual_dir/${fb_format}-idx${idx_pad}.png"
    echo "==> visual fb_format=$fb_format selected_index=$idx"
    write_launcher_env "idle" "" "$idx"
    restart_supervised_launcher "/tmp/${label}-visual-${idx_pad}.tsv"
    sleep 8
    "$MISTER" snapshot "$snap_dir" >/dev/null
    cp "$snap_dir/fb0.png" "$png_out"
    "$MISTER" get "$REMOTE_LOG" "$visual_dir/${fb_format}-idx${idx_pad}.log" >/dev/null || true
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

check_steady_wall_gate() {
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
      if (slow > 0 && allow != "1" && allow != "true" && allow != "yes" && allow != "on") {
        printf "%s steady wall gate failed: frames_after_30=%d slow_gt_16667=%d\n", name, n, slow > "/dev/stderr"
        exit 4
      }
      printf "%s steady_wall_gate frames_after_30=%d slow_gt_16667=%d allow=%s\n", name, n, slow + 0, allow
    }
  ' "$tsv"
}

summarize_preview_timing() {
  local name="$1" log="$2"
  awk -v name="$name" '
    /preview_trace (decoded|apply) / {
      total = read = decode = 0
      cache_hit = "unknown"
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "total_us") total = kv[2] + 0
        else if (kv[1] == "read_us") read = kv[2] + 0
        else if (kv[1] == "decode_us") decode = kv[2] + 0
        else if (kv[1] == "cache_hit") cache_hit = kv[2]
      }
      n++
      total_sum += total; read_sum += read; decode_sum += decode
      if (total > total_max) total_max = total
      if (read > read_max) read_max = read
      if (decode > decode_max) decode_max = decode
      if (cache_hit == "true") cache_hits++
      if (read > 0) file_reads++
      if (read > 5000) slow_reads++
    }
    END {
      if (n == 0) {
        printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name
      } else {
        printf "%s\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%d\t%d\t%d\n",
          name, n, total_sum / n, total_max, read_sum / n, read_max, decode_sum / n, decode_max,
          cache_hits + 0, file_reads + 0, slow_reads + 0
      }
    }
  ' "$log"
}

check_preview_hotpath_cache_gate() {
  local name="$1" log="$2"
  local failures
  failures="$(grep -c 'preview_trace cache_failed' "$log" 2>/dev/null || true)"
  if [[ "$failures" != "0" && "$allow_hotpath_misses" != "1" && "$allow_hotpath_misses" != "true" && "$allow_hotpath_misses" != "yes" && "$allow_hotpath_misses" != "on" ]]; then
    echo "$name preview hot-path cache gate failed: cache_failed=$failures" >&2
    return 5
  fi
  echo "$name preview_hotpath_cache_gate cache_failed=$failures allow=$allow_hotpath_misses"
}

check_preview_visibility_gate() {
  local name="$1" tsv="$2"
  awk -v name="$name" -v list_only="$list_only" -v allow="$allow_no_exact_preview" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      n++
      state = $(col["cache_state"])
      states[state]++
    }
    END {
      exact = states["exact"] + 0
      if (list_only != "1" && exact == 0 && allow != "1" && allow != "true" && allow != "yes" && allow != "on") {
        printf "%s preview visibility gate failed: exact=0 frames=%d allow=%s\n", name, n, allow > "/dev/stderr"
        exit 6
      }
      printf "%s preview_visibility_gate frames=%d exact=%d allow=%s list_only=%s\n", name, n, exact, allow, list_only
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

  list_only="0"
  allow_no_exact_preview="0"
  if check_preview_visibility_gate selftest "$no_exact" >/dev/null 2>&1; then
    echo "preview visibility self-test expected exact=0 failure" >&2
    rm -rf "$tmp"
    exit 1
  fi
  check_preview_visibility_gate selftest "$exact" >/dev/null

  allow_no_exact_preview="1"
  check_preview_visibility_gate selftest "$no_exact" >/dev/null

  list_only="1"
  allow_no_exact_preview="0"
  check_preview_visibility_gate selftest "$no_exact" >/dev/null

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
echo $'preview_timing\trows\tavg_total_us\tmax_total_us\tavg_read_us\tmax_read_us\tavg_decode_us\tmax_decode_us\tcache_hits\tfile_reads\tslow_reads'
summarize_preview_timing arcade "$arcade_log"

echo
check_steady_wall_gate arcade "$arcade_tsv"
check_preview_hotpath_cache_gate arcade "$arcade_log"
check_preview_visibility_gate arcade "$arcade_tsv"
