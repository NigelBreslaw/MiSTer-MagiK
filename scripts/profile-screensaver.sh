#!/usr/bin/env bash
# Run the full-screen RGB565 screensaver scene on the MiSTer and summarize frame pacing.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/screensaver-profiles"
REMOTE="/media/fat/mister-magik/mister-magik-fb"

usage() {
  cat <<'EOF'
Usage: scripts/profile-screensaver.sh [LABEL] [--skip-build|--deploy-device] [--mode mega|MODE] [--segment-secs N] [--secs N] [--fb-format 565] [--preview-format png|derived-png|raw-rgb|raw-rgb565] [--visual-captures N]

Runs:
  mister-magik-fb ui screensaver
with MISTER_SCREENSAVER_TRACE, summarizes overall and by screensaver mode, and
uses the same process-owner cleanup hygiene as preview-scroll benchmarks.
EOF
}

label="screensaver-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
mode="mega"
segment_secs="20"
secs=""
fb_format="565"
preview_format="raw-rgb565"
visual_captures="0"
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
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -ge 1 ]]; then label="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -gt 1 ]]; then usage >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ ! "$mode" =~ ^[A-Za-z0-9_,.-]+$ ]]; then echo "--mode must be a comma-separated screensaver label list or mega" >&2; exit 2; fi
if [[ ! "$segment_secs" =~ ^[0-9]+$ || "$segment_secs" -lt 1 ]]; then echo "--segment-secs must be a positive integer" >&2; exit 2; fi
case "$fb_format" in 565) ;; *) echo "--fb-format must be 565; RGB888 UI support was removed" >&2; exit 2 ;; esac
case "$preview_format" in png|derived-png|raw-rgb|raw-rgb565|raw565|rgb565|565) ;; *) echo "--preview-format must be png, derived-png, raw-rgb, or raw-rgb565" >&2; exit 2 ;; esac
if [[ ! "$visual_captures" =~ ^[0-9]+$ ]]; then echo "--visual-captures must be an integer" >&2; exit 2; fi

if [[ -z "$secs" ]]; then
  if [[ "$mode" == "mega" || "$mode" == "all" || "$mode" == "demo" ]]; then
    secs=$((segment_secs * 18))
  else
    IFS=',' read -r -a modes <<<"$mode"
    secs=$((segment_secs * ${#modes[@]}))
  fi
fi
if [[ ! "$secs" =~ ^[0-9]+$ || "$secs" -lt 1 ]]; then echo "--secs must be a positive integer" >&2; exit 2; fi

mkdir -p "$OUT_DIR"

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope arcade ;;
  skip) : ;;
esac

remote_tsv="/tmp/${label}-screensaver.tsv"
remote_log="/tmp/${label}-screensaver.log"
local_tsv="$OUT_DIR/${label}-screensaver.tsv"
local_log="$OUT_DIR/${label}-screensaver.log"

echo "==> screensaver label=$label mode=$mode secs=$secs segment_secs=$segment_secs fb_format=$fb_format preview_format=$preview_format"
"$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
rm -f '$remote_tsv' '$remote_log'
sleep 5
MISTER_FB_FORMAT='$fb_format' MISTER_PREVIEW_FORMAT='$preview_format' MISTER_SCREENSAVER='$mode' MISTER_SCREENSAVER_SEGMENT_SECS='$segment_secs' MISTER_SCREENSAVER_TRACE='$remote_tsv' '$REMOTE' ui screensaver '$secs' >'$remote_log' 2>&1 &
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
  echo "screensaver failed; see $local_log" >&2
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
MISTER_FB_FORMAT='$fb_format' MISTER_PREVIEW_FORMAT='$preview_format' MISTER_SCREENSAVER='$mode' MISTER_SCREENSAVER_SEGMENT_SECS='$segment_secs' '$REMOTE' ui screensaver 30 >/tmp/${label}-visual-${i}.log 2>&1 &
echo \$! >/tmp/${label}-visual-${i}.pid
" >/dev/null
    sleep $((8 + i * segment_secs))
    "$MISTER" snapshot "$snap_dir" >/dev/null
    cp "$snap_dir/fb0.png" "$png_out"
    "$MISTER" run "kill -9 \$(cat /tmp/${label}-visual-${i}.pid 2>/dev/null) 2>/dev/null || true; rm -f /tmp/${label}-visual-${i}.pid" >/dev/null || true
    echo "wrote $png_out"
  done
fi

summarize() {
  local tsv="$1"
  awk '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      n++
      wall = $(col["wall_us"]) + 0
      walls[n] = wall
      sum += wall
      if (wall > 16667) slow16++
      if (wall > 20000) slow20++
    }
    END {
      for (i = 1; i <= n; i++) for (j = i + 1; j <= n; j++) if (walls[j] < walls[i]) { tmp = walls[i]; walls[i] = walls[j]; walls[j] = tmp }
      p95 = int(n * 0.95); if (p95 < 1) p95 = 1; if (p95 > n) p95 = n
      p99 = int(n * 0.99); if (p99 < 1) p99 = 1; if (p99 > n) p99 = n
      printf "%d\t%.0f\t%d\t%d\t%d\t%d\n", n, n ? sum / n : 0, walls[p95] + 0, walls[p99] + 0, slow16 + 0, slow20 + 0
    }
  ' "$tsv"
}

summarize_by_mode() {
  local tsv="$1"
  awk '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      mode = $(col["mode"])
      idx = ++n[mode]
      wall = $(col["wall_us"]) + 0
      walls[mode, idx] = wall
      sum[mode] += wall
      if (wall > 16667) slow16[mode]++
      if (wall > 20000) slow20[mode]++
      if (!(mode in seen)) { seen[mode] = 1; order[++order_n] = mode }
    }
    END {
      for (oi = 1; oi <= order_n; oi++) {
        mode = order[oi]
        count = n[mode]
        for (i = 1; i <= count; i++) sorted[i] = walls[mode, i]
        for (i = 1; i <= count; i++) for (j = i + 1; j <= count; j++) if (sorted[j] < sorted[i]) { tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp }
        p95 = int(count * 0.95); if (p95 < 1) p95 = 1; if (p95 > count) p95 = count
        p99 = int(count * 0.99); if (p99 < 1) p99 = 1; if (p99 > count) p99 = count
        printf "%s\t%d\t%.0f\t%d\t%d\t%d\t%d\n", mode, count, sum[mode] / count, sorted[p95], sorted[p99], slow16[mode] + 0, slow20[mode] + 0
        delete sorted
      }
    }
  ' "$tsv"
}

echo
echo $'case\tframes\tavg_wall_us\tp95_wall_us\tp99_wall_us\tslow_gt_16_7ms\tslow_gt_20ms\trss_hwm_kb'
read -r frames avg p95 p99 slow16 slow20 < <(summarize "$local_tsv")
rss="$(sed -n 's/^rss_hwm_kb=//p' "$local_log" | tail -1)"
printf "screensaver\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" "$frames" "$avg" "$p95" "$p99" "$slow16" "$slow20" "${rss:-0}"

echo
echo $'mode\tframes\tavg_wall_us\tp95_wall_us\tp99_wall_us\tslow_gt_16_7ms\tslow_gt_20ms'
summarize_by_mode "$local_tsv"
