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
pull_optional /tmp/mister-magik-visual-samples.tsv visual-samples.tsv

if [ "$KEEP_ENABLED" -eq 0 ]; then
  echo "==> disable boot analytics flag"
  "$MISTER" run "rm -f '$FLAG'; sync"
else
  echo "==> leaving boot analytics flag enabled"
fi

echo "==> compact timeline"
if [ -s "$OUT/boot-analytics.tsv" ]; then
  python3 - "$OUT/boot-analytics.tsv" "$OUT/boot-summary.md" <<'PY'
import csv
import sys

path = sys.argv[1]
summary_path = sys.argv[2]
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

def first_event(name):
    matches = [item for item in rows if item[1].get("event") == name]
    return min(matches, key=lambda item: item[0]) if matches else None

def event_ms(name):
    item = first_event(name)
    return item[0] if item else None

def detail_value(details, key):
    prefix = key + "="
    for part in details.split():
        if part.startswith(prefix):
            return part[len(prefix):]
    return None

def delta(a, b):
    if a is None or b is None:
        return "n/a"
    return f"{b - a}ms"

process_start = event_ms("process_start")
init_for_menu = event_ms("init_for_menu")
spawn_start = event_ms("spawn_start")
forked = event_ms("forked")
video_on = event_ms("video_fb_enable_on")
run_ui_start = event_ms("run_ui_start")
display_open_ok = event_ms("display_open_ok")
initial_route = event_ms("initial_fb_enable_direct_done")
app_construct = event_ms("app_construct")
app_show = event_ms("app_show")
catalog_cache = event_ms("catalog_cache_load")
first_render = event_ms("first_render")
first_vsync = event_ms("first_vsync")
first_copy = event_ms("first_copy")
first_frame = event_ms("first_frame")
stable_frame = event_ms("stable_frame")

owners = [
    (ms, detail_value(row.get("details", ""), "reason"), detail_value(row.get("details", ""), "owner"), row.get("details", ""))
    for ms, row in rows
    if row.get("event") == "visible_owner"
]
visuals = [
    (ms, detail_value(row.get("details", ""), "label"), detail_value(row.get("details", ""), "class"), row.get("details", ""))
    for ms, row in rows
    if row.get("event") == "fb_visual_sample"
]
menu_bg = [
    (ms, detail_value(row.get("details", ""), "draw_kind"), row.get("details", ""))
    for ms, row in rows
    if row.get("event") == "video_menu_bg_done"
]

with open(summary_path, "w") as out:
    out.write("# Boot Visual Summary\n\n")
    out.write("## Phase Timings\n\n")
    out.write(f"- Main init to spawn: {delta(init_for_menu, spawn_start)}\n")
    out.write(f"- Spawn to forked child: {delta(spawn_start, forked)}\n")
    out.write(f"- Forked child to Main fb0 route: {delta(forked, video_on)}\n")
    out.write(f"- Slint process start to run_ui_start: {delta(process_start, run_ui_start)}\n")
    out.write(f"- run_ui_start to display_open_ok: {delta(run_ui_start, display_open_ok)}\n")
    out.write(f"- display_open_ok to Slint initial route: {delta(display_open_ok, initial_route)}\n")
    out.write(f"- run_ui_start to app_construct: {delta(run_ui_start, app_construct)}\n")
    out.write(f"- app_construct to app_show: {delta(app_construct, app_show)}\n")
    out.write(f"- app_show to first_render: {delta(app_show, first_render)}\n")
    out.write(f"- first_render to first_copy: {delta(first_render, first_copy)}\n")
    out.write(f"- first_copy to stable_frame: {delta(first_copy, stable_frame)}\n")
    out.write(f"- run_ui_start to first_frame: {delta(run_ui_start, first_frame)}\n")
    out.write(f"- run_ui_start to stable_frame: {delta(run_ui_start, stable_frame)}\n\n")

    out.write("## Visible Owner Transitions\n\n")
    if owners:
        last = None
        for ms, reason, owner, details in owners:
            pair = (owner, reason)
            if pair == last:
                continue
            last = pair
            out.write(f"- {ms}ms: owner={owner or 'unknown'} reason={reason or 'unknown'}\n")
    else:
        out.write("- No visible owner snapshots captured.\n")
    out.write("\n")

    out.write("## Menu Background Draws\n\n")
    if menu_bg:
        for ms, kind, details in menu_bg[:20]:
            out.write(f"- {ms}ms: draw_kind={kind or 'unknown'} {details}\n")
    else:
        out.write("- No menu background draws captured.\n")
    out.write("\n")

    out.write("## Framebuffer Visual Samples\n\n")
    if visuals:
        for ms, label, klass, details in visuals:
            out.write(f"- {ms}ms: {label or 'sample'} class={klass or 'unknown'} {details}\n")
    else:
        out.write("- No framebuffer visual samples captured.\n")
    out.write("\n")

    out.write("## Initial Interpretation Checklist\n\n")
    out.write("- Static source: inspect `Menu Background Draws` and early `Visible Owner Transitions`.\n")
    out.write("- Black source: compare owner=fb0/core/menu_bg with `Framebuffer Visual Samples` classes.\n")
    out.write("- Slowest phase: inspect `Phase Timings`, especially app_show to first_render and run_ui_start to first_frame.\n")
PY
  echo "==> derived summary: $OUT/boot-summary.md"
else
  echo "    no boot analytics TSV captured"
fi

echo "==> bundle: $OUT"
