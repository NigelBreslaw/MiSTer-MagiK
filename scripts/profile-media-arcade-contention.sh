#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Run the supported single-launcher media-download plus Arcade contention gate.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
args=()
secs=30
timeout_secs=420
timeout_explicit=0
self_test=0
correctness_only=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?--secs needs seconds}"; shift 2 ;;
    --timeout) timeout_secs="${2:?--timeout needs seconds}"; timeout_explicit=1; shift 2 ;;
    --scenario)
      [[ "${2:?--scenario needs a value}" == "human-turbo-hold" ]] || {
        echo "only the supported human-turbo-hold scenario is accepted" >&2
        exit 2
      }
      shift 2
      ;;
    --correctness-only) correctness_only=1; shift ;;
    --self-test) self_test=1; shift ;;
    *) args+=("$1"); shift ;;
  esac
done

if [[ ! "$timeout_secs" =~ ^[0-9]+$ || "$timeout_secs" -lt 1 || "$timeout_secs" -gt 600 ]]; then
  echo "--timeout must be an integer from 1 through 600 seconds" >&2
  exit 2
fi

if [[ "$self_test" -eq 1 ]]; then
  [[ "$timeout_secs" -eq 420 && "$timeout_explicit" -eq 0 ]]
  "$HERE/scripts/profile-media-cold-boot.sh" --self-test
  echo "profile-media-arcade-contention self-test ok"
  exit 0
fi

if [[ "$correctness_only" -eq 1 ]]; then
  args+=(--contention-correctness-only)
fi

exec "$HERE/scripts/profile-media-cold-boot.sh" "${args[@]}" \
  --keep-catalog --thread-sample --arcade-trace-secs "$secs" --timeout "$timeout_secs"
