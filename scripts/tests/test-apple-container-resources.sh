#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
. "$ROOT/magik-gui/scripts/apple-container-resources.sh"

status() {
  printf 'ID IMAGE STATE IP CPUS MEMORY\n'
  printf 'buildkit image %s 192.0.2.1/24 %s %s %s\n' "$1" "$2" "$3" "$4"
}

assert_silent() {
  local output
  output="$(apple_container_warn_builder_resources "$1" 10 2>&1)"
  [ -z "$output" ] || { printf 'unexpected warning: %s\n' "$output" >&2; exit 1; }
}

assert_warns() {
  local output
  output="$(apple_container_warn_builder_resources "$1" 10 2>&1)"
  printf '%s\n' "$output" | grep -q 'WARNING: Apple container builder has'
  printf '%s\n' "$output" | grep -q 'container builder stop'
  printf '%s\n' "$output" | grep -q 'container builder start --cpus "$(getconf _NPROCESSORS_ONLN)" --memory 8g'
}

assert_silent "$(status running 10 8192 MB)"
# Captured shape from Apple container CLI 1.0.0 on 2026-07-17.
assert_silent 'ID        IMAGE                                                STATE    IP                 CPUS  MEMORY
buildkit  ghcr.io/apple/container-builder-shim/builder:0.12.0  running  192.168.64.251/24  10    8192 MB'
assert_warns "$(status running 2 8192 MB)"
assert_warns "$(status running 10 2048 MB)"
assert_silent "$(status stopped 2 2048 MB)"
assert_silent 'ID IMAGE STATE IP CPUS MEMORY'
assert_silent 'buildkit unparseable'

echo 'Apple container resource checks ok'
