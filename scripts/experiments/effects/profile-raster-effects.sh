#!/usr/bin/env bash
# Experimental: run the full-screen classic raster/palette effects scene on the MiSTer and summarize frame pacing.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
HERE="$(experiment_repo_root)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/raster-effect-profiles"
RESULTS="$HERE/history/toolchain-bench/results-raster-effects.tsv"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
EFFECT_COUNT=15

usage() {
  cat <<'EOF'
Usage: scripts/experiments/effects/profile-raster-effects.sh [LABEL] [--skip-build|--deploy-device] [--mode mega|EFFECT[,EFFECT...]] [--segment-secs N] [--secs N] [--fb-format 565] [--preview-format png|derived-png|raw-rgb|raw-rgb565] [--visual-captures N] [--replace-label]

Runs the experimental scene:
  mister-magik-fb ui raster-effects
with MISTER_RASTER_EFFECTS_TRACE, summarizes overall and by raster effect, and
uses the same process-owner cleanup hygiene as preview/screensaver benchmarks.
EOF
}

label="raster-effects-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
mode="mega"
segment_secs="20"
secs=""
fb_format="565"
preview_format="raw-rgb565"
visual_captures="0"
replace_label="0"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --mode) mode="${2:-}"; shift 2 ;;
    --segment-secs) segment_secs="${2:-}"; shift 2 ;;
    --secs) secs="${2:-}"; shift 2 ;;
    --fb-format) fb_format="${2:-}"; shift 2 ;;
    --preview-format) preview_format="${2:-}"; shift 2 ;;
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    --replace-label) replace_label="1"; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -ge 1 ]]; then label="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -gt 1 ]]; then usage >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ ! "$mode" =~ ^[A-Za-z0-9_,.-]+$ ]]; then echo "--mode must be a comma-separated effect label list or mega" >&2; exit 2; fi
if [[ ! "$segment_secs" =~ ^[0-9]+$ || "$segment_secs" -lt 1 ]]; then echo "--segment-secs must be a positive integer" >&2; exit 2; fi
case "$fb_format" in 565) ;; *) echo "--fb-format must be 565; RGB888 UI support was removed" >&2; exit 2 ;; esac
case "$preview_format" in png|derived-png|raw-rgb|raw-rgb565|raw565|rgb565|565) ;; *) echo "--preview-format must be png, derived-png, raw-rgb, or raw-rgb565" >&2; exit 2 ;; esac
if [[ ! "$visual_captures" =~ ^[0-9]+$ ]]; then echo "--visual-captures must be an integer" >&2; exit 2; fi

if [[ -z "$secs" ]]; then
  if [[ "$mode" == "mega" || "$mode" == "all" || "$mode" == "demo" ]]; then
    secs=$((segment_secs * EFFECT_COUNT))
  else
    IFS=',' read -r -a selected_modes <<<"$mode"
    secs=$((segment_secs * ${#selected_modes[@]}))
  fi
fi
if [[ ! "$secs" =~ ^[0-9]+$ || "$secs" -lt 1 ]]; then echo "--secs must be a positive integer" >&2; exit 2; fi

mkdir -p "$OUT_DIR" "$(dirname "$RESULTS")"

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --experiments ;;
  skip) : ;;
esac
require_experiment_binary "$MISTER" "$REMOTE" "effect scene experiments"

HEADER="label	effect	frames	fps	avg_wall_us	p95_wall_us	p99_wall_us	slow_gt_16_7ms	slow_gt_20ms	avg_cpu_pct	p95_cpu_pct	max_trace_cpu_pct	max_sample_cpu_pct	avg_cpu_us	p95_cpu_us	avg_draw_us	p95_draw_us	avg_present_us	p95_present_us	avg_vsync_us	p95_vsync_us	avg_clear_us	p95_clear_us	avg_background_us	p95_background_us	avg_projection_us	p95_projection_us	avg_image_blit_us	p95_image_blit_us	avg_sprite_us	p95_sprite_us	avg_post_us	p95_post_us	avg_hud_us	p95_hud_us	avg_palette_step_count	p95_palette_step_count	max_palette_step_count	avg_lut_lookup_count	p95_lut_lookup_count	max_lut_lookup_count	avg_row_op_count	p95_row_op_count	max_row_op_count	avg_dither_pixel_count	p95_dither_pixel_count	max_dither_pixel_count	avg_flash_pixel_count	p95_flash_pixel_count	max_flash_pixel_count	avg_trail_pixel_count	p95_trail_pixel_count	max_trail_pixel_count	avg_indexed_pixel_count	p95_indexed_pixel_count	max_indexed_pixel_count	avg_reflection_row_count	p95_reflection_row_count	max_reflection_row_count	rss_hwm_kb	visual_ok	date	notes"
if [[ ! -f "$RESULTS" ]] || ! head -1 "$RESULTS" | grep -q $'^label\teffect'; then
  echo "$HEADER" >"$RESULTS"
fi
if [[ "$replace_label" == "1" ]]; then
  tmp_results="$(mktemp)"
  awk -v label="$label" 'NR == 1 || ($0 != "" && substr($0, 1, length(label) + 1) != label "\t")' "$RESULTS" >"$tmp_results"
  mv "$tmp_results" "$RESULTS"
fi

remote_tsv="/tmp/${label}-raster-effects.tsv"
remote_log="/tmp/${label}-raster-effects.log"
local_tsv="$OUT_DIR/${label}-raster-effects.tsv"
local_log="$OUT_DIR/${label}-raster-effects.log"

echo "==> raster-effects label=$label mode=$mode secs=$secs segment_secs=$segment_secs fb_format=$fb_format preview_format=$preview_format"
"$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
rm -f '$remote_tsv' '$remote_log'
sleep 5
MISTER_FB_FORMAT='$fb_format' MISTER_PREVIEW_FORMAT='$preview_format' MISTER_RASTER_EFFECTS='$mode' MISTER_RASTER_EFFECTS_AUTO=1 MISTER_RASTER_EFFECTS_SEGMENT_SECS='$segment_secs' MISTER_RASTER_EFFECTS_TRACE='$remote_tsv' '$REMOTE' ui raster-effects '$secs' >'$remote_log' 2>&1 &
UI_PID=\$!
RSS_MAX=0
CPU_SUM=0
CPU_MAX=0
CPU_N=0
TICKS=0
MAX_TICKS=$((secs + 15))
CLK=\$(getconf CLK_TCK 2>/dev/null || echo 100)
jiffies() { awk '{print \$14+\$15}' /proc/\$1/stat 2>/dev/null || echo 0; }
LAST_JIFFIES=\$(jiffies \$UI_PID)
while kill -0 \$UI_PID 2>/dev/null; do
  RSS=\$(awk '/^VmHWM:/{print \$2}' /proc/\$UI_PID/status 2>/dev/null || echo 0)
  case \"\$RSS\" in ''|*[!0-9]*) RSS=0 ;; esac
  [ \"\$RSS\" -gt \"\$RSS_MAX\" ] && RSS_MAX=\$RSS
  sleep 1
  NOW_JIFFIES=\$(jiffies \$UI_PID)
  DELTA=\$((NOW_JIFFIES - LAST_JIFFIES))
  LAST_JIFFIES=\$NOW_JIFFIES
  CPU=\$((DELTA * 100 / CLK))
  [ \"\$CPU\" -lt 0 ] 2>/dev/null && CPU=0
  CPU_SUM=\$((CPU_SUM + CPU))
  [ \"\$CPU\" -gt \"\$CPU_MAX\" ] && CPU_MAX=\$CPU
  CPU_N=\$((CPU_N + 1))
  TICKS=\$((TICKS + 1))
  if [ \"\$TICKS\" -ge \"\$MAX_TICKS\" ]; then
    echo bench_timeout_after_ticks=\$TICKS >>'$remote_log'
    kill -9 \$UI_PID 2>/dev/null || true
    break
  fi
done
UI_RC=0
wait \$UI_PID 2>/dev/null || UI_RC=\$?
echo rss_hwm_kb=\$RSS_MAX >>'$remote_log'
if [ \$CPU_N -gt 0 ]; then echo cpu_sample_mean_pct=\$((CPU_SUM / CPU_N)) >>'$remote_log'; else echo cpu_sample_mean_pct=0 >>'$remote_log'; fi
echo cpu_sample_max_pct=\$CPU_MAX >>'$remote_log'
if [ \"\$UI_RC\" -ne 0 ]; then
  echo ui_exit_status=\$UI_RC >>'$remote_log'
  exit \$UI_RC
fi
test -s '$remote_tsv'
" || {
  "$MISTER" get "$remote_log" "$local_log" || true
  echo "raster-effects failed; see $local_log" >&2
  exit 1
}

"$MISTER" get "$remote_tsv" "$local_tsv"
"$MISTER" get "$remote_log" "$local_log"
echo "wrote $local_tsv"
echo "wrote $local_log"

if [[ "$visual_captures" != "0" ]]; then
  visual_dir="$OUT_DIR/${label}-visuals"
  mkdir -p "$visual_dir"
  for ((i = 0; i < visual_captures; i++)); do
    png_out="$visual_dir/capture-${i}.png"
    snap_dir="$visual_dir/capture-${i}.snapshot"
    "$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
sleep 5
MISTER_FB_FORMAT='$fb_format' MISTER_PREVIEW_FORMAT='$preview_format' MISTER_RASTER_EFFECTS='$mode' MISTER_RASTER_EFFECTS_AUTO=1 MISTER_RASTER_EFFECTS_SEGMENT_SECS='$segment_secs' MISTER_RASTER_EFFECTS_HUD=1 '$REMOTE' ui raster-effects 30 >/tmp/${label}-visual-${i}.log 2>&1 &
echo \$! >/tmp/${label}-visual-${i}.pid
" >/dev/null
    sleep $((8 + i * segment_secs))
    "$MISTER" snapshot "$snap_dir" >/dev/null
    cp "$snap_dir/fb0.png" "$png_out"
    "$MISTER" run "kill -9 \$(cat /tmp/${label}-visual-${i}.pid 2>/dev/null) 2>/dev/null || true; rm -f /tmp/${label}-visual-${i}.pid" >/dev/null || true
    echo "wrote $png_out"
  done
fi

summarize_by_effect() {
  local tsv="$1" label="$2" rss="$3" sample_cpu_max="$4" visual_ok="$5" notes="$6"
  local run_date
  run_date="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  awk -v label="$label" -v rss="$rss" -v sample_cpu_max="$sample_cpu_max" -v visual_ok="$visual_ok" -v notes="$notes" -v run_date="$run_date" '
    BEGIN {
      FS="\t"; OFS="\t";
      metric_count = split("wall_us cpu_pct cpu_us draw_us present_us vsync_us clear_us background_us projection_us image_blit_us sprite_us post_us hud_us palette_step_count lut_lookup_count row_op_count dither_pixel_count flash_pixel_count trail_pixel_count indexed_pixel_count reflection_row_count", metrics, " ");
      counter_count = split("palette_step_count lut_lookup_count row_op_count dither_pixel_count flash_pixel_count trail_pixel_count indexed_pixel_count reflection_row_count", counters, " ");
    }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      effect = $(col["effect"])
      if (!(effect in seen)) { seen[effect] = 1; order[++order_n] = effect }
      idx = ++n[effect]
      for (m = 1; m <= metric_count; m++) {
        name = metrics[m]
        v = $(col[name]) + 0
        vals[effect, name, idx] = v
        sum[effect, name] += v
        if (name == "cpu_pct" && v > max_trace_cpu[effect]) max_trace_cpu[effect] = v
      }
      for (m = 1; m <= counter_count; m++) {
        name = counters[m]
        v = $(col[name]) + 0
        if (v > max_counter[effect, name]) max_counter[effect, name] = v
      }
      wall = $(col["wall_us"]) + 0
      if (wall > 16667) slow16[effect]++
      if (wall > 20000) slow20[effect]++
    }
    END {
      for (oi = 1; oi <= order_n; oi++) {
        effect = order[oi]
        count = n[effect]
        for (m = 1; m <= metric_count; m++) {
          name = metrics[m]
          for (i = 1; i <= count; i++) sorted[i] = vals[effect, name, i]
          for (i = 1; i <= count; i++) for (j = i + 1; j <= count; j++) if (sorted[j] < sorted[i]) { tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp }
          p95 = int(count * 0.95); if (p95 < 1) p95 = 1; if (p95 > count) p95 = count
          p99 = int(count * 0.99); if (p99 < 1) p99 = 1; if (p99 > count) p99 = count
          avg[effect, name] = count ? int(sum[effect, name] / count) : 0
          pct95[effect, name] = sorted[p95] + 0
          pct99[effect, name] = sorted[p99] + 0
          delete sorted
        }
        fps = sum[effect, "wall_us"] > 0 ? (count * 1000000.0 / sum[effect, "wall_us"]) : 0
        print label, effect, count, sprintf("%.1f", fps),
          avg[effect, "wall_us"], pct95[effect, "wall_us"], pct99[effect, "wall_us"], slow16[effect] + 0, slow20[effect] + 0,
          avg[effect, "cpu_pct"], pct95[effect, "cpu_pct"], max_trace_cpu[effect] + 0, sample_cpu_max,
          avg[effect, "cpu_us"], pct95[effect, "cpu_us"],
          avg[effect, "draw_us"], pct95[effect, "draw_us"],
          avg[effect, "present_us"], pct95[effect, "present_us"],
          avg[effect, "vsync_us"], pct95[effect, "vsync_us"],
          avg[effect, "clear_us"], pct95[effect, "clear_us"],
          avg[effect, "background_us"], pct95[effect, "background_us"],
          avg[effect, "projection_us"], pct95[effect, "projection_us"],
          avg[effect, "image_blit_us"], pct95[effect, "image_blit_us"],
          avg[effect, "sprite_us"], pct95[effect, "sprite_us"],
          avg[effect, "post_us"], pct95[effect, "post_us"],
          avg[effect, "hud_us"], pct95[effect, "hud_us"],
          avg[effect, "palette_step_count"], pct95[effect, "palette_step_count"], max_counter[effect, "palette_step_count"] + 0,
          avg[effect, "lut_lookup_count"], pct95[effect, "lut_lookup_count"], max_counter[effect, "lut_lookup_count"] + 0,
          avg[effect, "row_op_count"], pct95[effect, "row_op_count"], max_counter[effect, "row_op_count"] + 0,
          avg[effect, "dither_pixel_count"], pct95[effect, "dither_pixel_count"], max_counter[effect, "dither_pixel_count"] + 0,
          avg[effect, "flash_pixel_count"], pct95[effect, "flash_pixel_count"], max_counter[effect, "flash_pixel_count"] + 0,
          avg[effect, "trail_pixel_count"], pct95[effect, "trail_pixel_count"], max_counter[effect, "trail_pixel_count"] + 0,
          avg[effect, "indexed_pixel_count"], pct95[effect, "indexed_pixel_count"], max_counter[effect, "indexed_pixel_count"] + 0,
          avg[effect, "reflection_row_count"], pct95[effect, "reflection_row_count"], max_counter[effect, "reflection_row_count"] + 0,
          rss, visual_ok, run_date, notes
      }
    }
  ' "$tsv"
}

rss="$(sed -n 's/^rss_hwm_kb=//p' "$local_log" | tail -1)"
cpu_sample_max="$(sed -n 's/^cpu_sample_max_pct=//p' "$local_log" | tail -1)"
notes="fb_format=$fb_format; preview_format=$preview_format; mode=$mode; segment_secs=$segment_secs"
visual_ok="yes"
if [[ "$visual_captures" == "0" ]]; then
  visual_ok="not-run"
fi

summary_tmp="$(mktemp)"
summarize_by_effect "$local_tsv" "$label" "${rss:-0}" "${cpu_sample_max:-0}" "$visual_ok" "$notes" >"$summary_tmp"
cat "$summary_tmp" >>"$RESULTS"

echo
echo "$HEADER"
cat "$summary_tmp"
rm -f "$summary_tmp"
echo
echo "appended $RESULTS"
