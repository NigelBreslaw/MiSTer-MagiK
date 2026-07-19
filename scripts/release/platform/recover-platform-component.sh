#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Recover one exact component from the newest verified immutable platform release.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
COMPONENT=""
COMPONENT_ID=""
OUTPUT=""
GITHUB_OUTPUT_PATH=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --component) COMPONENT="${2:?missing component}"; shift 2 ;;
    --component-id) COMPONENT_ID="${2:?missing component ID}"; shift 2 ;;
    --output) OUTPUT="${2:?missing output}"; shift 2 ;;
    --github-output) GITHUB_OUTPUT_PATH="${2:?missing GitHub output path}"; shift 2 ;;
    *) echo "ERROR: unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ ! "$COMPONENT" =~ ^(main|fpga|kernel)$ || ! "$COMPONENT_ID" =~ ^[0-9a-f]{64}$ || -z "$OUTPUT" ]]; then
  echo "ERROR: --component, --component-id, and --output are required." >&2
  exit 2
fi
if [[ -e "$OUTPUT" ]]; then
  echo "ERROR: output already exists: $OUTPUT" >&2
  exit 2
fi
mkdir -p "$(dirname "$OUTPUT")"

TEMP="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-component-recovery.XXXXXX")"
trap 'rm -rf "$TEMP"' EXIT
gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/releases?per_page=100" > "$TEMP/releases.json"

write_outputs() {
  local hit="$1" result="${2:-}"
  [[ -n "$GITHUB_OUTPUT_PATH" ]] || return 0
  echo "hit=$hit" >> "$GITHUB_OUTPUT_PATH"
  if [[ "$hit" = true ]]; then
    python3 - "$result" >> "$GITHUB_OUTPUT_PATH" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
for key in ("run_id", "head_sha", "workflow", "head_branch", "release_version"):
    print(f"{key}={value[key]}")
PY
  fi
}

while IFS= read -r tag; do
  [[ -n "$tag" ]] || continue
  candidate="$TEMP/$tag"
  mkdir -p "$candidate"
  if ! gh release download "$tag" --repo "$GITHUB_REPOSITORY" --dir "$candidate" \
    --pattern 'platform-bundle-v0.*.json' 2>/dev/null; then
    continue
  fi
  manifest="$(find "$candidate" -name 'platform-bundle-v0.*.json' -print -quit)"
  [[ -n "$manifest" ]] || continue
  if ! python3 - "$manifest" "$COMPONENT" "$COMPONENT_ID" <<'PY'
import json, sys
keys = {"main": "main_input_sha256", "fpga": "fpga_input_sha256", "kernel": "kernel_input_sha256"}
try:
    payload = json.load(open(sys.argv[1]))
except (OSError, ValueError):
    raise SystemExit(1)
raise SystemExit(0 if payload.get(keys[sys.argv[2]]) == sys.argv[3] else 1)
PY
  then
    continue
  fi
  if ! gh release download "$tag" --repo "$GITHUB_REPOSITORY" --dir "$candidate" \
    --pattern 'mister-magik-platform-v0.*.zip' 2>/dev/null; then
    continue
  fi
  archive="$(find "$candidate" -name 'mister-magik-platform-v0.*.zip' -print -quit)"
  result="$candidate/origin.json"
  recovered="$candidate/recovered"
  if python3 "$ROOT/scripts/release/platform/platform-bundle.py" extract-component \
    "$archive" --manifest "$manifest" --component "$COMPONENT" \
    --component-id "$COMPONENT_ID" --output "$recovered" > "$result"; then
    mv "$recovered" "$OUTPUT"
    write_outputs true "$result"
    echo "Recovered $COMPONENT $COMPONENT_ID from $tag"
    exit 0
  fi
done < <(python3 "$ROOT/scripts/release/databases/select-published-release.py" \
  platform --all --releases "$TEMP/releases.json")

write_outputs false
echo "No published platform contains $COMPONENT $COMPONENT_ID"
