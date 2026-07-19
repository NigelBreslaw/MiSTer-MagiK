#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Helpers for scripts that temporarily run mister-magik-fb outside Main supervision.

mister_supervision_command() {
  local command="$1"
  local settle="${2:-0.5}"
  local mister_bin="${MISTER:-${HERE:-}/scripts/mister}"

  if [[ -z "$mister_bin" || "$mister_bin" == "/scripts/mister" ]]; then
    echo "mister_supervision_command requires MISTER or HERE" >&2
    return 2
  fi

  local action
  case "$command" in
    mister_magik_suspend) action=suspend ;;
    mister_magik_resume) action=resume ;;
    mister_magik_restart_launcher) action=restart-launcher ;;
    "load_core menu.rbf") action=return-to-launcher ;;
    *) echo "unsupported acknowledged Main command: $command" >&2; return 2 ;;
  esac
  "$mister_bin" agent magik "$action"
  sleep "$settle"
}

mister_suspend_launcher() {
  mister_supervision_command "mister_magik_suspend" "${1:-0.5}"
}

mister_restart_launcher() {
  mister_supervision_command "mister_magik_restart_launcher" "${1:-0.5}"
}
