#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

if [ "$#" -lt 3 ]; then
  echo "usage: $0 <profile> <features> <binary>" >&2
  exit 2
fi

PROFILE="$1"
FEATURES="$2"
BIN="$3"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
LOG="$ROOT/build/binary-size.tsv"

if [ ! -f "$BIN" ]; then
  echo "ERROR: binary missing: $BIN" >&2
  exit 1
fi

bytes() {
  stat -f%z "$1" 2>/dev/null || stat -c%s "$1"
}

human_bytes() {
  awk -v b="$1" 'BEGIN {
    split("B KiB MiB GiB", u, " ");
    n = b + 0;
    i = 1;
    while (n >= 1024 && i < 4) { n /= 1024; i++ }
    if (i == 1) printf "%d %s", n, u[i];
    else printf "%.2f %s", n, u[i];
  }'
}

BYTES="$(bytes "$BIN")"
mkdir -p "$(dirname "$LOG")"

if [ ! -f "$LOG" ]; then
  printf 'date\tprofile\tfeatures\tbytes\tdelta_bytes\tbinary\n' >"$LOG"
fi

PREV="$(
  awk -F '\t' -v profile="$PROFILE" -v features="$FEATURES" '
    NR > 1 && $2 == profile && $3 == features { bytes = $4 }
    END { if (bytes != "") print bytes }
  ' "$LOG"
)"

DELTA=""
DELTA_LABEL="n/a"
if [ -n "$PREV" ]; then
  DELTA=$((BYTES - PREV))
  if [ "$DELTA" -gt 0 ]; then
    DELTA_LABEL="+$(human_bytes "$DELTA")"
  elif [ "$DELTA" -lt 0 ]; then
    ABS_DELTA=$((-DELTA))
    DELTA_LABEL="-$(human_bytes "$ABS_DELTA")"
  else
    DELTA_LABEL="0 B"
  fi
fi

DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
  "$DATE" "$PROFILE" "$FEATURES" "$BYTES" "$DELTA" "$BIN" >>"$LOG"

echo "==> binary size"
echo "    profile:  $PROFILE"
echo "    features: ${FEATURES:-none}"
echo "    path:     $BIN"
echo "    bytes:    $BYTES ($(human_bytes "$BYTES"))"
echo "    delta:    $DELTA_LABEL vs previous same profile/features"
echo "    history:  $LOG"
