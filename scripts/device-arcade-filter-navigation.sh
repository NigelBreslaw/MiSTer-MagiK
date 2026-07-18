#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
ITERATIONS="${MISTER_ARCADE_FILTER_NAV_ITERATIONS:-3}"
WAIT_FRAMES="${MISTER_ARCADE_FILTER_NAV_WAIT_FRAMES:-60}"
TIMEOUT_SECS="${MISTER_ARCADE_FILTER_NAV_TIMEOUT_SECS:-30}"

cleanup() {
  "$MISTER" launcher-restart --clear-env --timeout "$TIMEOUT_SECS" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
  "$MISTER" launcher-restart \
    --env MISTER_LAUNCHER_START_SCREEN=arcade \
    --env MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES="$WAIT_FRAMES" \
    --env MISTER_LAUNCHER_INPUT_SCRIPT=left,left,down,down,a \
    --timeout "$TIMEOUT_SECS" >/dev/null

  status="$($MISTER run "sed -n '1p' /tmp/mister-magik/status.json 2>/dev/null || true")"
  STATUS_JSON="$status" python3 - "$iteration" <<'PY'
import json
import os
import sys

iteration = int(sys.argv[1])
status = json.loads(os.environ["STATUS_JSON"])
expected = {
    "screen": "arcade",
    "arcade_drawer_open": True,
    "arcade_drawer_level": "Decades",
    "arcade_drawer_selected": 0,
}
errors = [
    f"{key}={status.get(key)!r} expected={value!r}"
    for key, value in expected.items()
    if status.get(key) != value
]
requested_hash = int(status.get("arcade_drawer_requested_hash", 0))
rendered_hash = int(status.get("arcade_drawer_rendered_hash", 0))
if requested_hash == 0:
    errors.append("arcade_drawer_requested_hash is zero")
if rendered_hash != requested_hash:
    errors.append(
        f"arcade drawer surface identity mismatch: requested={requested_hash} rendered={rendered_hash}"
    )
if int(status.get("composition_recovery_count", 0)) != 0:
    errors.append(
        "composition recovery occurred: "
        f"count={status.get('composition_recovery_count')} "
        f"invariant={status.get('last_composition_invariant_kind')!r}"
    )
if errors:
    raise SystemExit(f"iteration {iteration}: " + "; ".join(errors))
print(
    "arcade_filter_navigation_tsv"
    f"\titeration={iteration}"
    "\tlevel=Decades"
    "\tselected=0"
    f"\trequested_hash={requested_hash}"
    f"\trendered_hash={rendered_hash}"
    "\tresult=pass"
)
PY
done

cleanup
trap - EXIT INT TERM
echo "device arcade filter navigation: PASS ($ITERATIONS iteration(s))"
