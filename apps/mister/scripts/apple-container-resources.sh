#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Shared local Apple-container resource policy: all online CPUs and 8 GiB RAM.

apple_container_cpus() {
  local cpus
  cpus="$(getconf _NPROCESSORS_ONLN 2>/dev/null || true)"
  case "$cpus" in
    ''|*[!0-9]*) ;;
    *) printf '%s\n' "$cpus"; return ;;
  esac

  cpus="$(sysctl -n hw.logicalcpu 2>/dev/null || true)"
  case "$cpus" in
    ''|*[!0-9]*) ;;
    *) printf '%s\n' "$cpus"; return ;;
  esac

  echo "ERROR: could not detect online CPU count for Apple container." >&2
  exit 1
}

apple_container_memory() {
  printf '8g\n'
}

apple_container_warn_builder_resources() {
  local status="${1:-}" desired_cpus="${2:-$(apple_container_cpus)}"
  local builder_line state actual_cpus memory_value memory_unit actual_memory_mb desired_memory_mb=8192

  builder_line="$(printf '%s\n' "$status" | awk '$1 == "buildkit" { print; exit }')"
  [ -n "$builder_line" ] || return 0
  [ "$(printf '%s\n' "$builder_line" | awk '{print NF}')" -ge 7 ] || return 0

  state="$(printf '%s\n' "$builder_line" | awk '{print $3}')"
  [ "$state" = running ] || return 0
  actual_cpus="$(printf '%s\n' "$builder_line" | awk '{print $(NF-2)}')"
  memory_value="$(printf '%s\n' "$builder_line" | awk '{print $(NF-1)}')"
  memory_unit="$(printf '%s\n' "$builder_line" | awk '{print $NF}')"
  case "$actual_cpus:$memory_value" in
    *[!0-9:]*|:*|*:) return 0 ;;
  esac
  case "$memory_unit" in
    MB) actual_memory_mb="$memory_value" ;;
    GB) actual_memory_mb=$((memory_value * 1024)) ;;
    *) return 0 ;;
  esac

  if [ "$actual_cpus" -ge "$desired_cpus" ] && [ "$actual_memory_mb" -ge "$desired_memory_mb" ]; then
    return 0
  fi

  printf 'WARNING: Apple container builder has %s CPUs and %s %s; MiSTer MagiK recommends %s CPUs and 8 GiB.\n' \
    "$actual_cpus" "$memory_value" "$memory_unit" "$desired_cpus" >&2
  printf '%s\n' 'Restart it when convenient:' >&2
  printf '%s\n' '  container builder stop' >&2
  printf '%s\n' '  container builder start --cpus "$(getconf _NPROCESSORS_ONLN)" --memory 8g' >&2
}
