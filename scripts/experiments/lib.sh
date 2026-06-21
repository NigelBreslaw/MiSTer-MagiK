#!/usr/bin/env bash
# Shared helpers for experimental MiSTer MagiK profiling scripts.

experiment_repo_root() {
  cd "$(dirname "${BASH_SOURCE[1]}")/../.." && pwd
}

require_experiment_binary() {
  local mister="$1"
  local remote="$2"
  local context="${3:-experiment script}"
  local out

  if ! out="$("$mister" run "test -x '$remote' && '$remote' experiment-capabilities" 2>&1)"; then
    echo "ERROR: unable to inspect deployed MiSTer MagiK binary for $context." >&2
    echo "       Build/deploy with experiments enabled, then retry." >&2
    echo "$out" >&2
    return 1
  fi

  if ! grep -q '^experiments=1$' <<<"$out"; then
    echo "ERROR: deployed MiSTer MagiK binary does not include experiments." >&2
    echo "       Build/deploy with: scripts/deploy-rust.sh --experiments" >&2
    echo "       Or run this script with --deploy-device." >&2
    return 1
  fi
}

require_preview_mega_transitions() {
  local mister="$1"
  local remote="$2"
  local out
  local count

  require_experiment_binary "$mister" "$remote" "preview transition experiments"
  out="$("$mister" run "'$remote' preview-transitions" 2>&1)"
  count="$( (grep -E '^[a-z0-9-]+$' <<<"$out" || true) | wc -l | tr -d ' ')"
  if [[ ! "$count" =~ ^[0-9]+$ || "$count" -le 1 ]]; then
    echo "ERROR: deployed binary does not expose experimental preview transitions." >&2
    echo "       Build/deploy with: scripts/deploy-rust.sh --experiments" >&2
    return 1
  fi
}
