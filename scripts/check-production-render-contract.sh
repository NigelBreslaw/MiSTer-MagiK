#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

status=0

check_absent() {
  local label="$1"
  local pattern="$2"
  shift 2
  local matches
  matches="$(rg -n --glob '!scripts/check-production-render-contract.sh' "$pattern" "$@" 2>/dev/null || true)"
  if [[ -n "$matches" ]]; then
    echo "render-contract violation: $label" >&2
    echo "$matches" >&2
    status=1
  fi
}

scope=(
  "magik-gui/src"
  "scripts"
  "docs"
  "AGENTS.md"
)

check_absent "retired framebuffer color format API" \
  "FramebufferFormat|Xrgb|XRGB|xrgb|MISTER_FB_FORMAT|MISTER_FB_RB" \
  "${scope[@]}"
check_absent "retired generic framebuffer open/mode APIs" \
  "open_with_format|write_mister_mode_format|fb_enable_format|buffer_mut\\(|buffer_u32_mut\\(" \
  "magik-gui/src"
check_absent "retired generic route abstraction" \
  "FpgaFramebufferRoute|for_ui_rgb565|for_plan_rgb565|fb-format-smoke" \
  "${scope[@]}"
check_absent "retired production present probes and diagnostic overlays" \
  "PresentProbe|EffectLabelOverlay|MISTER_PRESENT_PROBE|present_probe_us" \
  "${scope[@]}"

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi

echo "production render contract ok"
