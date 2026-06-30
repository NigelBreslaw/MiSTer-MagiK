#!/usr/bin/env bash
# Helpers for bounded writes to /dev/MiSTer_cmd from device scripts.

mister_fifo_remote_command() {
  local command="$1"
  local timeout="${2:-5}"
  printf "cmd_file=/dev/MiSTer_cmd; if [ ! -p \"\$cmd_file\" ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; ( printf '%%s\\\\n' %s > \"\$cmd_file\" ) & writer=\$!; waited=0; while kill -0 \"\$writer\" 2>/dev/null; do if [ \"\$waited\" -ge %q ]; then kill \"\$writer\" 2>/dev/null || true; wait \"\$writer\" 2>/dev/null || true; echo 'timeout writing %s to /dev/MiSTer_cmd'; exit 124; fi; sleep 1; waited=\$((waited + 1)); done; wait \"\$writer\"" \
    "$(printf "%q" "$command")" "$timeout" "$command"
}

mister_fifo_self_test() {
  local cmd
  cmd="$(mister_fifo_remote_command "load_core menu.rbf" 3)"
  case "$cmd" in
    *"/dev/MiSTer_cmd"**"load_core\\ menu.rbf"* ) ;;
    *) echo "bounded FIFO command did not include expected command" >&2; echo "$cmd" >&2; return 1 ;;
  esac
  case "$cmd" in
    *"exit 124"* ) ;;
    *) echo "bounded FIFO command did not include timeout failure" >&2; echo "$cmd" >&2; return 1 ;;
  esac
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -euo pipefail
  mister_fifo_self_test
  echo "mister-fifo-lib self-test ok"
fi
