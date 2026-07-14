#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="$ROOT/kernel/scanout-slots/mister_magik_scanout_slots.c"
UAPI="$ROOT/kernel/scanout-slots/mister_magik_scanout_slots_uapi.h"
PLATFORM="$ROOT/kernel/scanout-slots/mister_magik_scanout_platform.h"
POLICY="$ROOT/kernel/scanout-slots/mister_magik_scanout_policy.h"
RUST="$ROOT/magik-gui/src/framebuffer/scanout_slots.rs"
AGENT="$ROOT/tools/magik-agent/src/scanout_slots_contract.rs"
DOC="$ROOT/documentation/src/content/docs/architecture/kernel-scanout-plugin.mdx"
KO="$ROOT/build/scanout-slots/mister_magik_scanout_slots.ko"
DEPLOY="$ROOT/scripts/deploy-platform.sh"
INSTALL="$ROOT/scripts/install-slint-boot.sh"

require_text() {
  local file="$1" text="$2"
  if ! grep -Fq "$text" "$file"; then
    echo "scanout contract missing '$text' in ${file#$ROOT/}" >&2
    exit 1
  fi
}

for file in "$SOURCE" "$UAPI" "$PLATFORM" "$POLICY" "$RUST" "$AGENT" "$DOC"; do
  test -f "$file"
done
for text in \
  MISTER_MAGIK_PLATFORM_CONTRACT_ID \
  MISTER_MAGIK_PLATFORM_KERNEL_REVISION \
  MISTER_MAGIK_PLATFORM_FB_DRIVER_SHA256 \
  MISTER_MAGIK_PLATFORM_DT_SHA256 \
  MISTER_MAGIK_PLATFORM_SLOT0_PHYS \
  MISTER_MAGIK_PLATFORM_SLOT1_PHYS; do
  require_text "$PLATFORM" "$text"
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
source_sha256="$(cd "$ROOT/kernel/scanout-slots" && sha256sum mister_magik_scanout_slots.c mister_magik_scanout_slots_uapi.h mister_magik_scanout_platform.h mister_magik_scanout_policy.h Makefile | sha256sum | awk '{print $1}')"
[[ "$source_sha256" =~ ^[0-9a-f]{64}$ ]]
policy_test="$(mktemp "${TMPDIR:-/tmp}/mister-magik-scanout-policy.XXXXXX")"
trap 'rm -f "$policy_test"' EXIT
${CC:-cc} -std=c11 -Wall -Wextra -Werror \
  "$ROOT/kernel/scanout-slots/mister_magik_scanout_policy_test.c" -o "$policy_test"
"$policy_test"
for text in platform-v1.manifest platform_contract_sha256 scanout_module_sha256 latch_rbf_sha256; do
  require_text "$DEPLOY" "$text"
  require_text "$INSTALL" "$text"
done
for text in /dev/mister-magik-scanout-slots 960x540 RGB565 /dev/fb0 QEMU; do
  require_text "$DOC" "$text"
done

if rg -n 'probe_|kzalloc|kmalloc|dma_|mailbox|ownership|fence|cacheable|workqueue|INIT_WORK|timer_setup|hrtimer_|debugfs|proc_create|sysfs|ioremap|request_irq|free_irq' \
    "$SOURCE" "$UAPI" "$PLATFORM" "$POLICY"; then
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
