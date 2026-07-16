#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Download and verify the newest immutable game-database release from GitHub.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPOSITORY="${MISTER_MAGIK_GITHUB_REPOSITORY:-NigelBreslaw/MiSTer-MagiK}"

usage() {
  cat <<'EOF'
Usage: scripts/fetch-game-databases-release.sh OUTPUT_DIR

Downloads the highest published game-databases-vN release from GitHub,
verifies its archive, manifest, checksums, SQLite databases, and source
versions, then atomically creates OUTPUT_DIR with the extracted files.
OUTPUT_DIR must not already exist.
EOF
}

if [[ $# -ne 1 ]]; then
  usage
  exit 2
fi
if [[ "$1" == "-h" || "$1" == "--help" ]]; then
  usage
  exit 0
fi

OUTPUT_DIR="$1"
if [[ -e "$OUTPUT_DIR" ]]; then
  echo "ERROR: output path already exists: $OUTPUT_DIR" >&2
  exit 1
fi
for command in gh python3; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "ERROR: required command is unavailable: $command" >&2
    exit 1
  fi
done

parent="$(dirname "$OUTPUT_DIR")"
mkdir -p "$parent"
work="$(mktemp -d "$parent/.game-databases-release.XXXXXX")"
trap 'rm -rf "$work"' EXIT

echo "==> Selecting latest published game-database release from $REPOSITORY"
gh api --paginate --slurp "repos/$REPOSITORY/releases?per_page=100" > "$work/releases.json"
tag="$(python3 "$ROOT/scripts/select-published-release.py" game-databases --releases "$work/releases.json")"
version="$(python3 "$ROOT/scripts/select-published-release.py" game-databases --field version --releases "$work/releases.json")"

release="$work/release"
extracted="$work/extracted"
mkdir -p "$release"
echo "==> Downloading and verifying $tag"
gh release download "$tag" --repo "$REPOSITORY" --dir "$release" \
  --pattern "mister-magik-game-databases-v${version}.zip" \
  --pattern game-databases-manifest.json \
  --pattern SHA256SUMS
python3 "$ROOT/scripts/game-databases-bundle.py" extract-release \
  "$release" --output "$extracted" >/dev/null

mv "$extracted" "$OUTPUT_DIR"
echo "Prepared verified $tag in $OUTPUT_DIR"
