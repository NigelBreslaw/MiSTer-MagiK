#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Concise console summaries for diagnostics whose complete evidence is already
# stored in an artifact directory.

diagnostic_failure_summary() {
  local label="$1" artifact_dir="$2" main_status="${3:-}" launcher_log="${4:-}"
  printf 'FAIL: %s\n' "$label" >&2
  printf 'artifacts=%s\n' "$artifact_dir" >&2

  if [[ -n "$main_status" && -s "$main_status" ]]; then
    python3 - "$main_status" <<'PY' >&2 || true
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as source:
        data = json.load(source)
except Exception:
    raise SystemExit(0)
main = data.get("runtime", {}).get("main_status") or data.get("main_status") or data
fields = {
    "state": main.get("launcher_state", "unknown"),
    "pid": main.get("launcher_pid", 0),
    "generation": main.get("main_generation", 0),
    "crashes": main.get("crash_count", 0),
}
print("main " + " ".join(f"{key}={value}" for key, value in fields.items()))
for key in ("last_restart_error", "last_spawn_error", "last_crash_reason", "last_invariant_detail"):
    value = main.get(key)
    if value:
        print(f"{key}={value}")
        break
PY
  fi

  if [[ -n "$launcher_log" && -s "$launcher_log" ]]; then
    local relevant
    relevant="$(grep -Ei 'fail|error|crash|timeout|invariant|panic' "$launcher_log" | tail -25 || true)"
    if [[ -n "$relevant" ]]; then
      printf '%s\n' "$relevant" >&2
    else
      tail -10 "$launcher_log" >&2 || true
    fi
  fi
}

diagnostic_failure_notice() {
  local label="$1" artifact_dir="$2" error_file="${3:-}"
  printf 'FAIL: %s\n' "$label" >&2
  printf 'artifacts=%s\n' "$artifact_dir" >&2
  if [[ -n "$error_file" && -s "$error_file" ]]; then
    tail -20 "$error_file" >&2 || true
  fi
}

diagnostic_output_self_test() {
  local tmp output
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/diagnostic-output.XXXXXX")"
  printf '%s\n' '{"launcher_state":"LauncherCrashed","launcher_pid":0,"main_generation":7,"crash_count":2,"last_crash_reason":"exit 126"}' >"$tmp/main.json"
  printf '%s\n' 'ordinary line' 'ERROR failed to start' >"$tmp/launcher.log"
  output="$(diagnostic_failure_summary "launcher start" "$tmp" "$tmp/main.json" "$tmp/launcher.log" 2>&1)"
  [[ "$output" == *"FAIL: launcher start"* ]]
  [[ "$output" == *"state=LauncherCrashed"* ]]
  [[ "$output" == *"last_crash_reason=exit 126"* ]]
  [[ "$output" == *"ERROR failed to start"* ]]
  rm -rf "$tmp"
  echo "diagnostic output self-test ok"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  set -euo pipefail
  diagnostic_output_self_test
fi
