#!/usr/bin/env bash
# Capture an opt-in boot analytics bundle for the Main->Slint handoff flicker.
#
#   scripts/boot-analytics.sh
#   scripts/boot-analytics.sh --deploy
#   scripts/boot-analytics.sh --keep-enabled --settle 12
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
FLAG="/media/fat/mister-magik/boot-analytics.enabled"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="$ROOT/build/boot-analytics/$STAMP"
DEPLOY=0
KEEP_ENABLED=0
SETTLE_SECS=10

usage() {
  sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --deploy)
      DEPLOY=1
      shift
      ;;
    --keep-enabled)
      KEEP_ENABLED=1
      shift
      ;;
    --settle)
      SETTLE_SECS="${2:?--settle needs seconds}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

mkdir -p "$OUT"

if [ "$DEPLOY" -eq 1 ]; then
  echo "==> deploy Main fork + Slint child (--fast)"
  "$ROOT/scripts/deploy-main-mister-experiment.sh" --fast
fi

echo "==> enable boot analytics flag"
"$MISTER" run "mkdir -p /media/fat/mister-magik; : > '$FLAG'; sync"

echo "==> reboot and wait"
"$MISTER" reboot-wait

echo "==> settle ${SETTLE_SECS}s for launcher stabilization"
sleep "$SETTLE_SECS"

echo "==> capture device state"
"$MISTER" run "echo '=== ps ==='; ps; echo '=== fb mode ==='; cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true; echo '=== active vt ==='; cat /sys/class/tty/tty0/active 2>/dev/null || true; echo '=== pids ==='; pidof MiSTer 2>/dev/null || true; pidof MiSTer_MagiK 2>/dev/null || true; pidof mister-magik-fb 2>/dev/null || true" > "$OUT/device-state.txt" || true

pull_optional() {
  local remote="$1"
  local local_name="$2"
  if "$MISTER" get "$remote" "$OUT/$local_name" >/dev/null 2>&1; then
    echo "    pulled $remote -> $local_name"
  else
    echo "    missing $remote" | tee "$OUT/$local_name.missing" >/dev/null
  fi
}

echo "==> pull analytics files"
pull_optional /tmp/mister-magik-boot-analytics.tsv boot-analytics.tsv
pull_optional /tmp/mister-magik-slint.log slint.log
pull_optional /tmp/mister-magik-main.log main.log
pull_optional /tmp/mister-magik-frame-profile.tsv frame-profile.tsv
pull_optional /tmp/mister-magik-launcher-frame-profile.tsv launcher-frame-profile.tsv

if [ "$KEEP_ENABLED" -eq 0 ]; then
  echo "==> disable boot analytics flag"
  "$MISTER" run "rm -f '$FLAG'; sync"
else
  echo "==> leaving boot analytics flag enabled"
fi

echo "==> compact timeline"
if [ -s "$OUT/boot-analytics.tsv" ]; then
  python3 - "$OUT/boot-analytics.tsv" <<'PY'
import csv
import sys

path = sys.argv[1]
rows = []
with open(path, newline="") as f:
    for row in csv.DictReader(f, delimiter="\t"):
        try:
            boot_ms = int(row.get("boot_ms") or 0)
        except ValueError:
            boot_ms = 0
        rows.append((boot_ms, row))

for boot_ms, row in sorted(rows, key=lambda item: item[0]):
    source = row.get("source", "")
    event = row.get("event", "")
    pid = row.get("pid", "")
    details = row.get("details", "")
    print(f"{boot_ms:>9}ms  {source:<5} pid={pid:<6} {event:<34} {details}")
PY
else
  echo "    no boot analytics TSV captured"
fi

echo "==> bundle: $OUT"
