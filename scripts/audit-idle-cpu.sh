#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Audit MagiK settled idle CPU and memory on the MiSTer.
#
# Uses only scripts/mister device entrypoints. The script restarts the
# supervised launcher, samples /proc on-device, captures framebuffer PNGs via
# the MagiK agent, and clears launcher.env before exiting.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_DIR="${1:-$HERE/build/idle-audit/$STAMP}"
SAMPLE_SECS="${MISTER_IDLE_AUDIT_SAMPLE_SECS:-20}"
WAIT_SECS="${MISTER_IDLE_AUDIT_WAIT_SECS:-120}"
CPU_LIMIT="${MISTER_IDLE_AUDIT_CPU_LIMIT:-1.00}"

mkdir -p "$OUT_DIR"

cleanup() {
  "$MISTER" launcher-restart --clear-env --timeout 30 >/dev/null 2>&1 || true
}
trap cleanup EXIT

remote_status() {
  "$MISTER" run "sed -n '1p' /tmp/mister-magik/status.json 2>/dev/null || true"
}

wait_settled() {
  local label="$1"
  local screen="$2"
  local status_file="$OUT_DIR/$label-status.json"
  local deadline=$((SECONDS + WAIT_SECS))

  while (( SECONDS < deadline )); do
    remote_status >"$status_file"
    if grep -q "\"screen\":\"$screen\"" "$status_file" \
      && grep -q '"catalog_refresh_done":true' "$status_file" \
      && grep -q '"idle":true' "$status_file"; then
      if [[ "$screen" != "arcade" ]] || {
        grep -q '"arcade_selected":0' "$status_file" \
          && grep -q '"preview_cache_state":"exact"' "$status_file"
      }; then
        return 0
      fi
    fi
    sleep 1
  done

  echo "timeout waiting for settled $label; last status:" >&2
  cat "$status_file" >&2 || true
  return 1
}

sample_cpu() {
  local label="$1"
  local out="$OUT_DIR/$label-proc-sample.txt"
  "$MISTER" run "
pid=\$(pidof mister-magik-fb) || exit 11
hz=\$(getconf CLK_TCK 2>/dev/null || echo 100)
dur=$SAMPLE_SECS
echo sample_start pid=\$pid hz=\$hz dur=\$dur ts=\$(date +%s)
j0=\$(awk '{print \$14+\$15}' /proc/\$pid/stat)
rm -f /tmp/magik-idle-thread0 /tmp/magik-idle-thread1
for t in /proc/\$pid/task/*; do
  tid=\${t##*/}
  name=\$(cat \$t/comm)
  j=\$(awk '{print \$14+\$15}' \$t/stat)
  echo \$tid \$j \$name >> /tmp/magik-idle-thread0
done
sleep \$dur
j1=\$(awk '{print \$14+\$15}' /proc/\$pid/stat)
dj=\$((j1-j0))
pct100=\$((dj*10000/(hz*dur)))
printf 'process_cpu_pct_one_core=%d.%02d elapsed_s=%d jiffies=%d\n' \$((pct100/100)) \$((pct100%100)) \$dur \$dj
for t in /proc/\$pid/task/*; do
  tid=\${t##*/}
  name=\$(cat \$t/comm)
  j=\$(awk '{print \$14+\$15}' \$t/stat)
  echo \$tid \$j \$name >> /tmp/magik-idle-thread1
done
echo thread_jiffies_${SAMPLE_SECS}s
awk 'NR==FNR{j0[\$1]=\$2; next} {d=\$2-j0[\$1]; printf \"%d %s %s\n\", d, \$1, \$3}' /tmp/magik-idle-thread0 /tmp/magik-idle-thread1 | sort -nr | sed -n '1,12p'
echo memory
awk '/VmRSS|VmHWM|VmSize|RssAnon|RssFile|RssShmem|Threads/ {print}' /proc/\$pid/status
echo status_json
sed -n '1p' /tmp/mister-magik/status.json 2>/dev/null || true
" >"$out"
}

capture_snapshot() {
  local label="$1"
  local dir="$OUT_DIR/$label.snapshot"
  mkdir -p "$dir"
  "$MISTER" status --json >"$dir/status.json" 2>/dev/null || true
  "$MISTER" agent framebuffer-capture "$dir/fb0.png" --json "$dir/framebuffer.json" >"$OUT_DIR/$label-snapshot.out"
}

cpu_value() {
  awk -F= '/process_cpu_pct_one_core=/ {split($2, a, " "); print a[1]; exit}' "$1"
}

assert_cpu_under_limit() {
  local label="$1"
  local sample="$OUT_DIR/$label-proc-sample.txt"
  local cpu
  cpu="$(cpu_value "$sample")"
  if [[ -z "$cpu" ]]; then
    echo "missing CPU sample for $label" >&2
    return 1
  fi
  awk -v cpu="$cpu" -v limit="$CPU_LIMIT" 'BEGIN { exit !(cpu < limit) }' || {
    echo "$label idle CPU ${cpu}% is not below ${CPU_LIMIT}%" >&2
    return 1
  }
}

check_no_stale_arming_files() {
  local out="$OUT_DIR/stale-arming-files.txt"
  "$MISTER" run "ls -l /media/fat/mister-magik/launcher.env /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik/rebuild-on-next-boot /media/fat/mister-magik-dev/rebuild-on-next-boot 2>/dev/null || true" >"$out"
  if [[ -s "$out" ]]; then
    echo "stale arming files remain:" >&2
    cat "$out" >&2
    return 1
  fi
}

echo "idle audit output: $OUT_DIR"

echo "==> Home idle"
"$MISTER" launcher-restart --clear-env --timeout 30
wait_settled home home
capture_snapshot home
sample_cpu home
assert_cpu_under_limit home

echo "==> Arcade idle"
"$MISTER" launcher-restart \
  --env MISTER_LAUNCHER_START_SCREEN=arcade \
  --env MISTER_LAUNCHER_LOCK_SCREEN=arcade \
  --timeout 30
wait_settled arcade arcade
capture_snapshot arcade
sample_cpu arcade
assert_cpu_under_limit arcade

echo "==> Cleanup"
"$MISTER" launcher-restart --clear-env --timeout 30
check_no_stale_arming_files

home_cpu="$(cpu_value "$OUT_DIR/home-proc-sample.txt")"
arcade_cpu="$(cpu_value "$OUT_DIR/arcade-proc-sample.txt")"
echo "idle_audit_summary home_cpu_pct=$home_cpu arcade_cpu_pct=$arcade_cpu out=$OUT_DIR"
