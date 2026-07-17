#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Canonical persistent and volatile files that can arm destructive launcher
# behaviour. Callers retain their own explicit cleanup traps; this library owns
# only the path set and the idempotent clear/verification operations.

arming_state_paths() {
  printf '%s\n' \
    /media/fat/mister-magik/launcher.env \
    /media/fat/mister-magik-dev/launcher.env \
    /tmp/mister-magik/fs-fault-launcher.env \
    /tmp/mister-magik/fs-fault-session \
    /tmp/mister-magik/fs-fault.json \
    /media/fat/mister-magik/rebuild-on-next-boot \
    /media/fat/mister-magik-dev/rebuild-on-next-boot
}

arming_state_remote_clear_command() {
  local paths
  paths="$(arming_state_paths | tr '\n' ' ')"
  printf 'rm -f %s; sync' "$paths"
}

arming_state_remote_report_command() {
  local paths
  paths="$(arming_state_paths | tr '\n' ' ')"
  printf 'for path in %s; do if [ -e "$path" ]; then ls -ld "$path"; fi; done' "$paths"
}

arming_state_clear() {
  local mister="$1"
  "$mister" run "$(arming_state_remote_clear_command)"
}

arming_state_report() {
  local mister="$1"
  "$mister" run "$(arming_state_remote_report_command)"
}

arming_state_report_is_clean() {
  [[ -z "${1//[[:space:]]/}" ]]
}

arming_state_assert_clean() {
  local mister="$1" report="${2:-}" output
  output="$(arming_state_report "$mister" 2>/dev/null)" || return 1
  if [[ -n "$report" ]]; then
    printf '%s\n' "$output" >"$report"
  fi
  arming_state_report_is_clean "$output"
}

arming_state_self_test() {
  local expected actual
  expected=$'/media/fat/mister-magik/launcher.env\n/media/fat/mister-magik-dev/launcher.env\n/tmp/mister-magik/fs-fault-launcher.env\n/tmp/mister-magik/fs-fault-session\n/tmp/mister-magik/fs-fault.json\n/media/fat/mister-magik/rebuild-on-next-boot\n/media/fat/mister-magik-dev/rebuild-on-next-boot'
  actual="$(arming_state_paths)"
  [[ "$actual" == "$expected" ]]
  arming_state_report_is_clean ""
  if arming_state_report_is_clean "/tmp/mister-magik/fs-fault-session"; then
    return 1
  fi
  while IFS= read -r path; do
    [[ "$(arming_state_remote_clear_command)" == *"$path"* ]]
    [[ "$(arming_state_remote_report_command)" == *"$path"* ]]
  done <<<"$actual"
  echo "arming state self-test ok"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    --self-test) arming_state_self_test ;;
    *) echo "usage: $0 --self-test" >&2; exit 2 ;;
  esac
fi
