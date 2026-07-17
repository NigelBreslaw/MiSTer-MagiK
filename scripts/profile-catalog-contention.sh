#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Extended human-turbo gate with measured catalog CPU overlap.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SECS=120
LABEL="catalog-contention-$(date -u +%Y%m%dT%H%M%SZ)"
PROFILE_ARGS=()

usage() {
  echo "usage: scripts/profile-catalog-contention.sh [LABEL] [--secs N] [profile-arcade-scroll options] [--self-test]"
}

if [[ "${1:-}" == "--self-test" ]]; then
  python3 "$ROOT/scripts/checks/check-catalog-contention.py" --self-test
  exit 0
fi
if [[ $# -gt 0 && "${1:-}" != --* ]]; then
  LABEL="$1"
  shift
fi
while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs)
      [[ $# -ge 2 ]] || { echo "--secs needs a value" >&2; exit 2; }
      SECS="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --scenario|--thread-sample|--catalog-refresh)
      echo "$1 is fixed by the catalog contention gate" >&2
      exit 2
      ;;
    *)
      PROFILE_ARGS+=("$1")
      shift
      ;;
  esac
done
[[ "$SECS" =~ ^[0-9]+$ ]] && [[ "$SECS" -ge 120 ]] || {
  echo "catalog contention evidence requires at least 120 seconds" >&2
  exit 2
}

MISTER_CATALOG_CONTENTION_QUIET_PREVIEWS=1 \
"$ROOT/scripts/profile-arcade-scroll.sh" "$LABEL" \
  --secs "$SECS" \
  --scenario human-turbo-hold \
  --catalog-refresh force \
  --thread-sample \
  --skip-search-overlap-gate \
  --skip-preview-exact-gate \
  "${PROFILE_ARGS[@]}"

python3 "$ROOT/scripts/checks/check-catalog-contention.py" \
  "$LABEL" \
  "$ROOT/build/arcade-scroll-profiles/${LABEL}-arcade-scroll.tsv" \
  "$ROOT/build/arcade-scroll-profiles/${LABEL}-arcade-scroll-thread-sample.tsv" \
  600 10
