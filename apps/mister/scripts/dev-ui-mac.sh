#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MISTER_DIR="$(cd "$HERE/.." && pwd)"
export RUSTC_WRAPPER=""

watch=0
release=1
while true; do
  case "${1:-}" in
    --watch)
      watch=1
      shift
      ;;
    --debug)
      release=0
      shift
      ;;
    *)
      break
      ;;
  esac
done

if [ "$release" = 1 ]; then
  profile_args=(--release)
  export SLINT_EMIT_DEBUG_INFO="${SLINT_EMIT_DEBUG_INFO:-0}"
else
  profile_args=()
  export SLINT_EMIT_DEBUG_INFO="${SLINT_EMIT_DEBUG_INFO:-1}"
fi

if [ "$watch" = 1 ]; then
  if ! cargo watch --version >/dev/null 2>&1; then
    echo "cargo-watch is required for --watch; run without --watch for a single preview session." >&2
    exit 2
  fi
  command="run"
  if [ "$release" = 1 ]; then
    command="$command --release"
  fi
  command="$command --manifest-path $MISTER_DIR/Cargo.toml --bin mister-magik-ui-preview --features ui-preview,experiments"
  if [ "$#" -gt 0 ]; then
    printf -v arguments ' %q' "$@"
    command="$command --$arguments"
  fi
  exec cargo watch \
    --watch "$MISTER_DIR/src" \
    --watch "$MISTER_DIR/ui" \
    --watch "$MISTER_DIR/ui-generated" \
    -x "$command"
fi

exec cargo run \
  "${profile_args[@]}" \
  --manifest-path "$MISTER_DIR/Cargo.toml" \
  --bin mister-magik-ui-preview \
  --features ui-preview,experiments \
  -- "$@"
