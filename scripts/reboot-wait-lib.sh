#!/usr/bin/env bash
# Shared reboot handling for install/restore scripts.

mister_reboot_wait_with_raw_fallback() {
  local mister_cmd="${MISTER:-scripts/mister}"
  echo "==> Reboot to apply"
  if "$mister_cmd" reboot-wait; then
    return 0
  fi
  echo "WARN: supervised reboot-wait did not observe a full down/up transition; retrying with raw reboot-wait" >&2
  "$mister_cmd" reboot-wait --raw
}

reboot_wait_lib_self_test() {
  local tmp fake log
  tmp="$(mktemp -d)"
  log="$tmp/calls.log"
  fake="$tmp/mister"
  cat >"$fake" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$MISTER_REBOOT_TEST_LOG"
if [ "$*" = "reboot-wait" ]; then
  exit 1
fi
if [ "$*" = "reboot-wait --raw" ]; then
  exit 0
fi
exit 2
EOF
  chmod +x "$fake"
  MISTER="$fake" MISTER_REBOOT_TEST_LOG="$log" mister_reboot_wait_with_raw_fallback >/dev/null
  if [ "$(sed -n '1p' "$log")" != "reboot-wait" ] || [ "$(sed -n '2p' "$log")" != "reboot-wait --raw" ]; then
    echo "reboot fallback did not call supervised then raw" >&2
    cat "$log" >&2
    rm -rf "$tmp"
    return 1
  fi
  rm -rf "$tmp"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -euo pipefail
  reboot_wait_lib_self_test
  echo "reboot-wait-lib self-test ok"
fi
