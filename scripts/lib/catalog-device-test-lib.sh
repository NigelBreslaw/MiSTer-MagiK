#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Shared lifecycle operations for destructive catalog acceptance runners.
# Scenario mutation and cleanup traps remain in each runner.

catalog_device_remote() { "$MISTER" run "$1"; }
catalog_device_db() { "$MISTER" db "$@"; }
catalog_device_last_line() { awk 'NF { value=$0 } END { print value }' | tr -d '\r'; }
catalog_device_last_number() { awk 'NF { value=$NF } END { gsub(/[^0-9]/, "", value); print value }'; }
catalog_device_shell_quote() { printf "'%s'" "$(printf "%s" "$1" | sed "s/'/'\\\\''/g")"; }
catalog_device_sql_string() { printf "'%s'" "$(printf "%s" "$1" | sed "s/'/''/g")"; }

catalog_device_wait_remote() {
  local label="$1" timeout="$2" command="$3" deadline=$((SECONDS + timeout))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if catalog_device_remote "$command" >/dev/null 2>&1; then
      echo "ok: $label"
      return 0
    fi
    sleep 1
  done
  fail "timeout waiting for $label"
}

catalog_device_assert_remote() {
  local label="$1" command="$2"
  if ! catalog_device_remote "$command" >/dev/null 2>&1; then
    fail "$label"
  fi
  echo "ok: $label"
}

catalog_device_restart_launcher() {
  local action="${1:-}"
  write_launcher_env "$action"
  catalog_device_remote "rm -f $(catalog_device_shell_quote "$REMOTE_LOG") $(catalog_device_shell_quote "$REMOTE_EVENTS") $(catalog_device_shell_quote "$REMOTE_STATUS"); if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\\n' > /dev/MiSTer_cmd"
  catalog_device_wait_remote "launcher process" 25 "test \"\$(ps w | grep '[m]ister-magik-fb ui launcher' | wc -l)\" = 1"
}

catalog_device_force_refresh() {
  local log="$1"
  catalog_device_remote "$(catalog_device_shell_quote "$REMOTE_BIN") library-refresh >$(catalog_device_shell_quote "$log") 2>&1"
}

catalog_device_restore_launcher_env() {
  if [ "$HAD_ENV" -eq 1 ] && [ -n "$ENV_BACKUP" ]; then
    catalog_device_remote "mv $(catalog_device_shell_quote "$ENV_BACKUP") $(catalog_device_shell_quote "$REMOTE_ENV")" >/dev/null 2>&1 || true
  else
    catalog_device_remote "rm -f $(catalog_device_shell_quote "$REMOTE_ENV")" >/dev/null 2>&1 || true
  fi
}

catalog_device_test_self_test() {
  [[ "$(printf 'one\ntwo\n' | catalog_device_last_line)" == two ]]
  [[ "$(printf 'value=42\n' | catalog_device_last_number)" == 42 ]]
  [[ "$(catalog_device_shell_quote "a'b")" == "'a'\\''b'" ]]
  [[ "$(catalog_device_sql_string "a'b")" == "'a''b'" ]]
  echo "catalog device test library self-test ok"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    --self-test) catalog_device_test_self_test ;;
    *) echo "usage: $0 --self-test" >&2; exit 2 ;;
  esac
fi
