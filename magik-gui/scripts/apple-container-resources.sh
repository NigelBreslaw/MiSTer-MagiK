#!/usr/bin/env bash
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
