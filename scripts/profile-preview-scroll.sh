#!/usr/bin/env bash
# Run a real arcade preview-scroll benchmark on the MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-scroll-profiles"
REMOTE="/media/fat/mister-magik/mister-magik-fb"

usage() {
  cat <<'EOF'
Usage: scripts/profile-preview-scroll.sh [SECS] [SCENARIO] [LABEL] [--skip-build|--deploy-fast|--deploy-device] [--list-only] [--fb-format 8888|565] [--preview-blitter slint|raw] [--transition EFFECT|mega] [--transition-segment-secs N] [--transition-ms N] [--visual-captures N] [--preview-visual-pct N] [--preview-resize-filter off|nearest|box|lanczos|hybrid] [--preview-resize-max 320x320] [--preview-format png|derived-png|raw-rgb|raw-rgb565]

Scenarios: velocity-scroll | held-scroll | turbo-hold | screenshot-stress
Runs the real launcher-backed arcade screen:
  ui arcade
with MISTER_LAUNCHER_BENCH_SCENARIO and MISTER_PREVIEW_SCROLL_TRACE.

--list-only disables screenshot loading and the real launcher's catalog refresh
worker so list-renderer changes can be measured without preview/catalog noise.
--fb-format selects the framebuffer format passed to the UI.
--preview-blitter selects Slint Image rendering or the raw post-render blitter.
--transition selects raw-preview screenshot transitions. Default is fade; `cut`
disables animation; `mega` cycles all effects. --transition-segment-secs controls
the benchmark window per effect.
--visual-captures captures fixed arcade indices from the real arcade screen after the
benchmark, storing before/after PNG evidence under <label>-visuals.
--preview-visual-pct scales screenshot display area. 100 is the current size;
50 renders screenshots at half the current visual area.
--preview-resize-filter enables runtime resize before Slint image creation.
--preview-resize-max sets the resize target box; default runtime code uses 320x320.
--preview-format selects original PNG, derived resized PNG, raw RGB, or raw RGB565 cache.

Do not use row-step scenarios such as list-scroll/smooth-scroll for arcade
performance benchmarking. They do not reproduce real velocity scrolling.
EOF
}

secs="30"
scenario="velocity-scroll"
label="preview-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
list_only="0"
preview_visual_pct=""
preview_resize_filter=""
preview_resize_max=""
preview_format=""
fb_format="8888"
preview_blitter="slint"
transition=""
transition_segment_secs=""
transition_ms=""
visual_captures="4"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-fast) deploy="fast"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --list-only) list_only="1"; shift ;;
    --fb-format) fb_format="${2:-}"; shift 2 ;;
    --preview-blitter) preview_blitter="${2:-}"; shift 2 ;;
    --transition) transition="${2:-}"; shift 2 ;;
    --transition-segment-secs) transition_segment_secs="${2:-}"; shift 2 ;;
    --transition-ms) transition_ms="${2:-}"; shift 2 ;;
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    --preview-visual-pct) preview_visual_pct="${2:-}"; shift 2 ;;
    --preview-resize-filter) preview_resize_filter="${2:-}"; shift 2 ;;
    --preview-resize-max) preview_resize_max="${2:-}"; shift 2 ;;
    --preview-format) preview_format="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -ge 1 ]]; then secs="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -ge 2 ]]; then scenario="${positionals[1]}"; fi
if [[ "${#positionals[@]}" -ge 3 ]]; then label="${positionals[2]}"; fi
if [[ "${#positionals[@]}" -gt 3 ]]; then usage >&2; exit 2; fi

preview_stress="0"
case "$scenario" in
  velocity-scroll|held-scroll|turbo-hold) ;;
  screenshot-stress|screenshot_stress|preview-stress|preview_stress)
    preview_stress="1"
    ;;
  list-scroll|smooth-scroll|selected-first|stress-scroll|cache-warm|preview-changes)
    echo "row-step/jump scenario '$scenario' is not valid for preview scroll benchmarking; use velocity-scroll or turbo-hold" >&2
    exit 2
    ;;
  *) echo "unknown scenario: $scenario" >&2; usage >&2; exit 2 ;;
esac
remote_scenario="$scenario"
if [[ "$remote_scenario" == "velocity-scroll" ]]; then
  # Keep the human-facing name explicit while sending the concrete scenario name
  # understood by both old and new deployed binaries.
  remote_scenario="held-scroll"
fi
if [[ "$preview_stress" == "1" ]]; then
  remote_scenario="stress-scroll"
fi
if [[ ! "$secs" =~ ^[0-9]+$ ]]; then echo "secs must be an integer" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ -n "$preview_visual_pct" && ! "$preview_visual_pct" =~ ^[0-9]+$ ]]; then echo "--preview-visual-pct must be an integer" >&2; exit 2; fi
case "$preview_resize_filter" in ""|off|nearest|box|lanczos|hybrid) ;; *) echo "--preview-resize-filter must be off, nearest, box, lanczos, or hybrid" >&2; exit 2 ;; esac
if [[ -n "$preview_resize_max" && ! "$preview_resize_max" =~ ^[0-9]+[xX][0-9]+$ ]]; then echo "--preview-resize-max must look like 320x320" >&2; exit 2; fi
case "$preview_format" in ""|png|derived-png|raw-rgb|raw-rgb565|raw565|rgb565|565) ;; *) echo "--preview-format must be png, derived-png, raw-rgb, or raw-rgb565" >&2; exit 2 ;; esac
case "$fb_format" in 8888|565) ;; *) echo "--fb-format must be 8888 or 565" >&2; exit 2 ;; esac
case "$preview_blitter" in slint|raw) ;; *) echo "--preview-blitter must be slint or raw" >&2; exit 2 ;; esac
if [[ -n "$transition" && ! "$transition" =~ ^[A-Za-z0-9_,.-]+$ ]]; then echo "--transition must be a comma-separated transition label list or mega" >&2; exit 2; fi
if [[ -n "$transition_segment_secs" && ! "$transition_segment_secs" =~ ^[0-9]+$ ]]; then echo "--transition-segment-secs must be an integer" >&2; exit 2; fi
if [[ -n "$transition_ms" && ! "$transition_ms" =~ ^[0-9]+$ ]]; then echo "--transition-ms must be an integer" >&2; exit 2; fi
if [[ ! "$visual_captures" =~ ^[0-9]+$ ]]; then echo "--visual-captures must be an integer" >&2; exit 2; fi

mkdir -p "$OUT_DIR"

remote_extra_env="MISTER_FB_FORMAT=$fb_format MISTER_PREVIEW_BLITTER=$preview_blitter MISTER_CATALOG_REFRESH=off"
if [[ "$list_only" == "1" ]]; then
  remote_extra_env="MISTER_FB_FORMAT=$fb_format MISTER_PREVIEW_BLITTER=$preview_blitter MISTER_PREVIEW_LOADING=off MISTER_CATALOG_REFRESH=off"
fi
if [[ "$preview_stress" == "1" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_STRESS=1 MISTER_CATALOG_REFRESH=off"
fi
if [[ -n "$preview_visual_pct" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_VISUAL_PCT=$preview_visual_pct"
fi
if [[ -n "$preview_resize_filter" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_RESIZE_FILTER=$preview_resize_filter"
fi
if [[ -n "$preview_resize_max" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_RESIZE_MAX=$preview_resize_max"
fi
if [[ -n "$preview_format" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_FORMAT=$preview_format"
fi
if [[ -n "$transition" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_TRANSITION=$transition"
fi
if [[ -n "$transition_segment_secs" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_TRANSITION_SEGMENT_SECS=$transition_segment_secs"
fi
if [[ -n "$transition_ms" ]]; then
  remote_extra_env="$remote_extra_env MISTER_PREVIEW_TRANSITION_MS=$transition_ms"
fi
if [[ -z "$preview_format" && -z "$preview_resize_filter" && -z "$preview_resize_max" ]]; then
  run_label="default derived-png nearest 320x320"
else
  run_label="${preview_format:-app-default} ${preview_resize_filter:-app-default} resize ${preview_resize_max:-320x320}"
fi
remote_extra_env="$remote_extra_env MISTER_PREVIEW_RUN_LABEL='${run_label}'"

case "$deploy" in
  fast) "$HERE/scripts/deploy-rust.sh" --fast --ui-scope arcade ;;
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope arcade ;;
  skip) : ;;
esac

run_case() {
  local name="$1"
  local scene="$2"
  local remote_tsv="/tmp/${label}-${name}.tsv"
  local remote_log="/tmp/${label}-${name}.log"
  local local_tsv="$OUT_DIR/${label}-${name}.tsv"
  local local_log="$OUT_DIR/${label}-${name}.log"

  echo "==> $name scene=$scene scenario=$scenario remote_scenario=$remote_scenario secs=$secs fb_format=$fb_format preview_blitter=$preview_blitter transition=${transition:-fade} list_only=$list_only preview_stress=$preview_stress preview_visual_pct=${preview_visual_pct:-100} preview_resize_filter=${preview_resize_filter:-app-default} preview_resize_max=${preview_resize_max:-app-default} preview_format=${preview_format:-app-default}"
  "$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
rm -f '$remote_tsv' '$remote_log'
sleep 5
$remote_extra_env MISTER_LAUNCHER_BENCH_SCENARIO='$remote_scenario' MISTER_PREVIEW_TRACE=1 MISTER_PREVIEW_SCROLL_TRACE='$remote_tsv' '$REMOTE' ui '$scene' '$secs' >'$remote_log' 2>&1 &
UI_PID=\$!
RSS_MAX=0
TICKS=0
MAX_TICKS=$((secs + 15))
while kill -0 \$UI_PID 2>/dev/null; do
  RSS=\$(awk '/^VmHWM:/{print \$2}' /proc/\$UI_PID/status 2>/dev/null || echo 0)
  case \"\$RSS\" in ''|*[!0-9]*) RSS=0 ;; esac
  [ \"\$RSS\" -gt \"\$RSS_MAX\" ] && RSS_MAX=\$RSS
  sleep 1
  TICKS=\$((TICKS + 1))
  if [ \"\$TICKS\" -ge \"\$MAX_TICKS\" ]; then
    echo bench_timeout_after_ticks=\$TICKS >>'$remote_log'
    kill -9 \$UI_PID 2>/dev/null || true
    break
  fi
done
wait \$UI_PID 2>/dev/null || true
echo rss_hwm_kb=\$RSS_MAX >>'$remote_log'
test -s '$remote_tsv'
" || {
    "$MISTER" get "$remote_log" "$local_log" || true
    echo "$name failed; see $local_log" >&2
    exit 1
  }
  "$MISTER" get "$remote_tsv" "$local_tsv"
  "$MISTER" get "$remote_log" "$local_log"
  echo "wrote $local_tsv"
  echo "wrote $local_log"
}

capture_visuals() {
  local count="$visual_captures"
  if [[ "$count" == "0" || "$list_only" == "1" ]]; then
    return
  fi
  local visual_dir="$OUT_DIR/${label}-visuals"
  mkdir -p "$visual_dir"
  local indices=(0 7 14 21 28 35 42 49)
  local i idx idx_pad remote_log remote_pid snap_dir png_out
  for ((i = 0; i < count && i < ${#indices[@]}; i++)); do
    idx="${indices[$i]}"
    idx_pad="$(printf "%03d" "$idx")"
    remote_log="/tmp/${label}-visual-${fb_format}-${idx_pad}.log"
    remote_pid="/tmp/${label}-visual-${fb_format}-${idx_pad}.pid"
    snap_dir="$visual_dir/${fb_format}-idx${idx_pad}.snapshot"
    png_out="$visual_dir/${fb_format}-idx${idx_pad}.png"
    echo "==> visual fb_format=$fb_format preview_blitter=$preview_blitter selected_index=$idx"
    "$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
rm -f '$remote_log' '$remote_pid'
sleep 5
$remote_extra_env MISTER_LAUNCHER_BENCH_SCENARIO=idle MISTER_ARCADE_SELECTED_INDEX='$idx' MISTER_PREVIEW_TRACE=1 '$REMOTE' ui arcade 20 >'$remote_log' 2>&1 &
echo \$! >'$remote_pid'
" >/dev/null
    sleep 8
    "$MISTER" snapshot "$snap_dir" >/dev/null
    cp "$snap_dir/fb0.png" "$png_out"
    "$MISTER" get "$remote_log" "$visual_dir/${fb_format}-idx${idx_pad}.log" >/dev/null || true
    "$MISTER" run "kill -9 \$(cat '$remote_pid' 2>/dev/null) 2>/dev/null || true; rm -f '$remote_pid'" >/dev/null || true
    echo "wrote $png_out"
  done
}

summarize_trace() {
  local name="$1"
  local tsv="$2"
  local log="$3"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      next
    }
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
      if (n == 0) {
        printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name
        exit
      }
      for (i = 1; i <= n; i++) {
        for (j = i + 1; j <= n; j++) {
          if (walls[j] < walls[i]) {
            tmp = walls[i]; walls[i] = walls[j]; walls[j] = tmp
          }
        }
      }
      p95_i = int(n * 0.95)
      if (p95_i < 1) p95_i = 1
      if (p95_i > n) p95_i = n
      printf "%s\t%d\t%.0f\t%d\t%d\t%d\t%d\t%d\t%d\t%d\n",
        name, n, sum / n, walls[p95_i], slow16, slow20,
        states["exact"] + 0, states["cached"] + 0,
        states["stale"] + 0, states["placeholder"] + 0
    }
  ' "$tsv" | while IFS=$'\t' read -r n frames avg p95 slow16 slow20 exact cached stale placeholder; do
    local rss
    rss="$(sed -n 's/^rss_hwm_kb=//p' "$log" | tail -1)"
    printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
      "$n" "$frames" "$avg" "$p95" "$slow16" "$slow20" "$exact" "$cached" "$stale" "$placeholder" "${rss:-0}"
  done
}

summarize_trace_by_effect() {
  local tsv="$1"
  awk '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      has_effect = ("transition_effect" in col)
      next
    }
    NF && has_effect {
      effect = $(col["transition_effect"])
      idx = ++n[effect]
      wall = $(col["wall_us"]) + 0
      walls[effect, idx] = wall
      sum[effect] += wall
      if (wall > 16667) slow16[effect]++
      if (wall > 20000) slow20[effect]++
      if (!(effect in seen)) {
        seen[effect] = 1
        order[++order_n] = effect
      }
    }
    END {
      if (!has_effect) exit
      for (oi = 1; oi <= order_n; oi++) {
        effect = order[oi]
        count = n[effect]
        for (i = 1; i <= count; i++) {
          sorted[i] = walls[effect, i]
        }
        for (i = 1; i <= count; i++) {
          for (j = i + 1; j <= count; j++) {
            if (sorted[j] < sorted[i]) {
              tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp
            }
          }
        }
        p95_i = int(count * 0.95)
        if (p95_i < 1) p95_i = 1
        if (p95_i > count) p95_i = count
        printf "%s\t%d\t%.0f\t%d\t%d\t%d\n",
          effect, count, sum[effect] / count, sorted[p95_i],
          slow16[effect] + 0, slow20[effect] + 0
        delete sorted
      }
    }
  ' "$tsv"
}

check_velocity_motion() {
  local name="$1"
  local tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      next
    }
    NF {
      n++
      vi = $(col["visual_index"]) + 0
      frac = vi - int(vi)
      if (frac < 0) frac = -frac
      if (frac > 0.001 && frac < 0.999) fractional++
      if (seen) {
        delta = vi - last
        if (delta < 0) delta = -delta
        if (delta > 0.001) moving++
      }
      last = vi
      seen = 1
    }
    END {
      printf "%s\t%d\t%d\t%d\n", name, n, fractional, moving
      if (moving > 0 && fractional == 0) exit 3
    }
  ' "$tsv"
}

run_case arcade arcade
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
if [[ "$preview_stress" == "1" ]]; then
  check_velocity_motion arcade "$arcade_tsv" || true
else
  check_velocity_motion arcade "$arcade_tsv"
fi

echo
echo "preview trace counts:"
printf "arcade decoded=%s apply=%s\n" \
  "$(grep -c 'preview_trace decoded' "$arcade_log" 2>/dev/null || true)" \
  "$(grep -c 'preview_trace apply' "$arcade_log" 2>/dev/null || true)"
