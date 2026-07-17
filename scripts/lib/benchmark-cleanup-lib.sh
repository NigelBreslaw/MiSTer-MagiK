#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Shared, idempotent cleanup traps for production benchmark runners.

benchmark_cleanup_lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$benchmark_cleanup_lib_dir/arming-state-lib.sh"

benchmark_cleanup_callback=""
benchmark_cleanup_running=0

benchmark_cleanup_status() {
  local original_status="$1" cleanup_status="$2" signal="${3:-EXIT}"
  case "$signal" in
    INT) printf '130\n'; return ;;
    TERM) printf '143\n'; return ;;
  esac
  if [[ "$original_status" -ne 0 ]]; then
    printf '%s\n' "$original_status"
  elif [[ "$cleanup_status" -ne 0 ]]; then
    printf '%s\n' "$cleanup_status"
  else
    printf '0\n'
  fi
}

benchmark_cleanup_report_is_clean() {
  arming_state_report_is_clean "$1"
}

benchmark_cleanup_assert_no_arming_files() {
  arming_state_assert_clean "$1" "${2:-}"
}

benchmark_cleanup_clear_launcher_env() {
  local mister="$1" timeout="${2:-30}"
  "$mister" launcher-restart --clear-env --timeout "$timeout" >/dev/null
}

benchmark_cleanup_dispatch() {
  local original_status="$1" signal="${2:-EXIT}" cleanup_status=0 final_status
  if [[ "$benchmark_cleanup_running" == "1" ]]; then
    return
  fi
  benchmark_cleanup_running=1
  trap - EXIT INT TERM
  set +e
  if [[ -n "$benchmark_cleanup_callback" ]]; then
    "$benchmark_cleanup_callback" "$original_status" "$signal"
    cleanup_status=$?
  fi
  final_status="$(benchmark_cleanup_status "$original_status" "$cleanup_status" "$signal")"
  exit "$final_status"
}

benchmark_cleanup_install() {
  benchmark_cleanup_callback="$1"
  benchmark_cleanup_running=0
  trap 'benchmark_cleanup_dispatch $? EXIT' EXIT
  trap 'benchmark_cleanup_dispatch 130 INT' INT
  trap 'benchmark_cleanup_dispatch 143 TERM' TERM
}

benchmark_cleanup_self_test() {
  [[ "$(benchmark_cleanup_status 0 0 EXIT)" == "0" ]]
  [[ "$(benchmark_cleanup_status 7 0 EXIT)" == "7" ]]
  [[ "$(benchmark_cleanup_status 0 9 EXIT)" == "9" ]]
  [[ "$(benchmark_cleanup_status 7 9 EXIT)" == "7" ]]
  [[ "$(benchmark_cleanup_status 0 0 INT)" == "130" ]]
  [[ "$(benchmark_cleanup_status 0 0 TERM)" == "143" ]]
  benchmark_cleanup_report_is_clean ""
  benchmark_cleanup_report_is_clean $' \n\t'
  if benchmark_cleanup_report_is_clean "/media/fat/mister-magik/launcher.env"; then
    return 1
  fi
  echo "benchmark cleanup self-test ok"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    --self-test) benchmark_cleanup_self_test ;;
    *) echo "usage: $0 --self-test" >&2; exit 2 ;;
  esac
fi
