#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="$ROOT/kernel/scanout-slots/mister_magik_scanout_slots.c"
UAPI="$ROOT/kernel/scanout-slots/mister_magik_scanout_slots_uapi.h"
RUST="$ROOT/magik-gui/src/framebuffer/scanout_slots.rs"
AGENT="$ROOT/tools/magik-agent/src/main.rs"
DOC="$ROOT/documentation/src/content/docs/architecture/kernel-scanout-plugin.mdx"
KO="$ROOT/build/scanout-slots/mister_magik_scanout_slots.ko"

require_text() {
  local file="$1" text="$2"
  if ! grep -Fq "$text" "$file"; then
    echo "scanout contract missing '$text' in ${file#$ROOT/}" >&2
    exit 1
  fi
}

for file in "$SOURCE" "$UAPI" "$RUST" "$AGENT" "$DOC"; do
  test -f "$file"
done

for text in \
  MISTER_MAGIK_SCANOUT_SLOTS_GET_LAYOUT \
  MISTER_MAGIK_SCANOUT_SLOTS_SLOT_COUNT \
  MISTER_MAGIK_SCANOUT_SLOTS_LAYOUT_WRITE_COMBINE; do
  require_text "$UAPI" "$text"
done
for text in 0x227e_9000 0x22fd_2000 1_040_384; do
  require_text "$RUST" "$text"
  require_text "$AGENT" "$text"
done
if [[ "$(sha256sum "$UAPI" | awk '{print $1}')" != \
      "2d8dffc12b76c7346cbd6291ece440e824719ebb6b564192e3ba3f692eb8c5b9" ]]; then
  echo "scanout UAPI changed without updating the qualified contract" >&2
  exit 1
fi
source_sha256="$(cd "$ROOT/kernel/scanout-slots" && sha256sum mister_magik_scanout_slots.c mister_magik_scanout_slots_uapi.h Makefile | sha256sum | awk '{print $1}')"
if [[ "$source_sha256" != "2bd6b3cc4bc4718cbe7db18f88e10c0f5a37585821a9c47075ea4c65adef92fc" ]]; then
  echo "scanout kernel source changed without updating the qualified contract" >&2
  exit 1
fi
for text in /dev/mister-magik-scanout-slots 960x540 RGB565 /dev/fb0 QEMU; do
  require_text "$DOC" "$text"
done

if rg -n 'probe_|kzalloc|kmalloc|dma_|mailbox|ownership|fence|cacheable|workqueue|INIT_WORK|timer_setup|hrtimer_|debugfs|proc_create|sysfs|ioremap|request_irq|free_irq' \
    "$SOURCE" "$UAPI"; then
  echo "forbidden scanout-slot kernel surface found" >&2
  exit 1
fi
if rg -n 'HiddenRgb565Framebuffer|MISTER_PLUGIN_MAP_BANDWIDTH|hidden-dev-mem|framebuffer::hidden' \
    "$ROOT/magik-gui/src"; then
  echo "retired direct hidden-framebuffer path found" >&2
  exit 1
fi

if [[ -f "$KO" ]]; then
  audit_dir="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-scanout-audit.XXXXXX")"
  trap 'rm -rf "$audit_dir"' EXIT
  strings "$KO" >"$audit_dir/strings.txt"
  if command -v arm-linux-gnueabihf-nm >/dev/null 2>&1; then
    arm-linux-gnueabihf-nm -u "$KO" >"$audit_dir/imports.txt"
  else
    nm -u "$KO" >"$audit_dir/imports.txt"
  fi
  if rg -n '/dev/mister-magik-plugin-probe|/dev/mister-magik-scanout$|plugin_probe|MISTER_MAGIK_SCANOUT_(POST|ACQUIRE|RELEASE|SYNC)|mailbox|cacheable' \
      "$audit_dir/strings.txt" ||
     rg -n '(^|[[:space:]])(dma_|ioremap|request_irq|queue_work|schedule_work|hrtimer_|timer_setup)' \
      "$audit_dir/imports.txt"; then
    echo "forbidden scanout-slot binary surface found" >&2
    exit 1
  fi
fi

echo "scanout slots contract: ok"
