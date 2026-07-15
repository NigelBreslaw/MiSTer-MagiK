#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Run the supported single-launcher media-download plus Arcade contention gate.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
args=()
secs=30

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?--secs needs seconds}"; shift 2 ;;
    --scenario)
      [[ "${2:?--scenario needs a value}" == "human-turbo-hold" ]] || {
        echo "only the supported human-turbo-hold scenario is accepted" >&2
        exit 2
      }
      shift 2
      ;;
    --self-test) exec "$HERE/scripts/profile-media-cold-boot.sh" --self-test ;;
    *) args+=("$1"); shift ;;
  esac
done

exec "$HERE/scripts/profile-media-cold-boot.sh" "${args[@]}" \
  --keep-catalog --thread-sample --arcade-trace-secs "$secs"
