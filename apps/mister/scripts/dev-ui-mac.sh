#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MISTER_DIR="$(cd "$HERE/.." && pwd)"
export RUSTC_WRAPPER=""
export SLINT_EMIT_DEBUG_INFO="${SLINT_EMIT_DEBUG_INFO:-1}"

watch=0
if [ "${1:-}" = "--watch" ]; then
  watch=1
  shift
fi

if [ "$watch" = 1 ]; then
  if ! cargo watch --version >/dev/null 2>&1; then
    echo "cargo-watch is required for --watch; run without --watch for a single preview session." >&2
    exit 2
  fi
  command="run --manifest-path $MISTER_DIR/Cargo.toml --bin mister-magik-ui-preview --features ui-preview"
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
  --manifest-path "$MISTER_DIR/Cargo.toml" \
  --bin mister-magik-ui-preview \
  --features ui-preview \
  -- "$@"
