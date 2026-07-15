#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$here/.." && pwd)"
out_dir="$here/public/screenshots"
meta_dir="$out_dir/meta"

mkdir -p "$out_dir" "$meta_dir"

captures=(
  "home|Home screen with system tiles"
  "arcade-list|Arcade list with a visible preview"
  "search|Search keyboard or results pane"
  "settings|Settings screen"
  "controller-test|Controller Test screen"
  "controller-setup|Controller setup overlay"
  "catalog-scan|Catalog scan or update progress"
  "media-progress|Screenshot/media pack download popup"
  "launch-loading|Game launch loading overlay"
  "confirm-dialog|A confirmation dialog"
)

capture_one() {
  local slug="$1"
  local label="$2"
  local png="$out_dir/${slug}.png"
  local json="$meta_dir/${slug}.json"

  printf '\nNavigate the MiSTer to: %s\n' "$label"
  printf 'Press Return to capture %s, or Ctrl-C to stop. ' "$slug"
  read -r _
  "$repo_root/scripts/mister" agent framebuffer-capture "$png" --json "$json"
  printf 'Captured %s and %s\n' "$png" "$json"
}

for item in "${captures[@]}"; do
  IFS='|' read -r slug label <<<"$item"
  capture_one "$slug" "$label"
done
