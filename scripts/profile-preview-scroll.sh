#!/usr/bin/env bash
# Run paired preview-scroll benchmarks on the MiSTer and compare traces.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-scroll-profiles"
REMOTE="/media/fat/mister-magik/mister-magik-fb"

usage() {
  cat <<'EOF'
Usage: scripts/profile-preview-scroll.sh [SECS] [SCENARIO] [LABEL] [--skip-build|--deploy-fast|--deploy-device]

Scenarios: velocity-scroll | held-scroll | turbo-hold
Runs both:
  ui preview_scroll_bench
  ui launcher
with matching MISTER_LAUNCHER_BENCH_SCENARIO and MISTER_PREVIEW_SCROLL_TRACE.

Do not use row-step scenarios such as list-scroll/smooth-scroll for arcade
performance benchmarking. They do not reproduce real velocity scrolling.
EOF
}

secs="30"
scenario="velocity-scroll"
label="preview-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-fast) deploy="fast"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -ge 1 ]]; then secs="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -ge 2 ]]; then scenario="${positionals[1]}"; fi
if [[ "${#positionals[@]}" -ge 3 ]]; then label="${positionals[2]}"; fi
if [[ "${#positionals[@]}" -gt 3 ]]; then usage >&2; exit 2; fi

case "$scenario" in
  velocity-scroll|held-scroll|turbo-hold) ;;
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
if [[ ! "$secs" =~ ^[0-9]+$ ]]; then echo "secs must be an integer" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi

mkdir -p "$OUT_DIR"

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

  echo "==> $name scene=$scene scenario=$scenario remote_scenario=$remote_scenario secs=$secs"
  "$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
rm -f '$remote_tsv' '$remote_log'
sleep 5
MISTER_LAUNCHER_BENCH_SCENARIO='$remote_scenario' MISTER_PREVIEW_TRACE=1 MISTER_PREVIEW_SCROLL_TRACE='$remote_tsv' '$REMOTE' ui '$scene' '$secs' >'$remote_log' 2>&1 &
UI_PID=\$!
RSS_MAX=0
while kill -0 \$UI_PID 2>/dev/null; do
  RSS=\$(awk '/^VmHWM:/{print \$2}' /proc/\$UI_PID/status 2>/dev/null || echo 0)
  case \"\$RSS\" in ''|*[!0-9]*) RSS=0 ;; esac
  [ \"\$RSS\" -gt \"\$RSS_MAX\" ] && RSS_MAX=\$RSS
  sleep 1
done
wait \$UI_PID
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

run_case standalone preview_scroll_bench
run_case real launcher

standalone_tsv="$OUT_DIR/${label}-standalone.tsv"
standalone_log="$OUT_DIR/${label}-standalone.log"
real_tsv="$OUT_DIR/${label}-real.tsv"
real_log="$OUT_DIR/${label}-real.log"

echo
echo $'case\tframes\tavg_wall_us\tp95_wall_us\tslow_gt_16_7ms\tslow_gt_20ms\texact\tcached\tstale\tplaceholder\trss_hwm_kb'
summarize_trace standalone "$standalone_tsv" "$standalone_log"
summarize_trace real "$real_tsv" "$real_log"

echo
echo $'motion_check\tframes\tfractional_visual_index_frames\tmoving_frames'
check_velocity_motion standalone "$standalone_tsv"
check_velocity_motion real "$real_tsv"

echo
echo "preview trace counts:"
printf "standalone decoded=%s apply=%s\n" \
  "$(grep -c 'preview_trace decoded' "$standalone_log" 2>/dev/null || true)" \
  "$(grep -c 'preview_trace apply' "$standalone_log" 2>/dev/null || true)"
printf "real       decoded=%s apply=%s\n" \
  "$(grep -c 'preview_trace decoded' "$real_log" 2>/dev/null || true)" \
  "$(grep -c 'preview_trace apply' "$real_log" 2>/dev/null || true)"
