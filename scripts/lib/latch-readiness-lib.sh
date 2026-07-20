#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Prove or restore the manifest-owned FPGA latch path. Callers must select a
# layout with magik_layout_select before using these helpers.

latch_readiness_probe() {
  local mister="$1"
  local app="${MISTER_MAGIK_APP_DIR:?select a MagiK layout first}"
  local main_name="${MISTER_MAGIK_MAIN_NAME:?select a MagiK layout first}"
  local latch_rbf="$app/fpga/menu-magik-vblank-latch.rbf"
  local bin="$app/mister-magik-fb"

  "$mister" run "
pid=\$(pidof '$main_name') || { echo 'latch_readiness_tsv valid=0 reason=main-not-running'; exit 20; }
cmdline=\$(tr '\\000' ' ' < \"/proc/\$pid/cmdline\")
case \"\$cmdline\" in *'$latch_rbf'*) ;; *) echo \"latch_readiness_tsv valid=0 reason=wrong-main-core cmdline=\$cmdline\"; exit 21 ;; esac
grep -q '^mister_magik_scanout_slots ' /proc/modules || { echo 'latch_readiness_tsv valid=0 reason=scanout-module-missing'; exit 22; }
test -c /dev/mister-magik-scanout-slots || { echo 'latch_readiness_tsv valid=0 reason=scanout-device-missing'; exit 23; }
report=\$('$bin' latch-readiness-report) || { printf '%s\\n' "\$report"; echo 'latch_readiness_tsv valid=0 state=runtime-fault stage=live-probe reason=latch-readiness-command-failed'; exit 24; }
printf '%s\\n' \"\$report\"
printf '%s\\n' \"\$report\" | grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready' || { echo 'latch_readiness_tsv valid=0 state=runtime-fault stage=live-probe reason=readiness-proof-missing'; exit 25; }
"
}

latch_readiness_is_contract_failure() {
  case "$1" in
    *"latch_readiness_tsv valid=0 "*) return 0 ;;
    *) return 1 ;;
  esac
}

latch_readiness_summary() {
  local report="$1" line detail
  line="$(printf '%s\n' "$report" | grep 'latch_readiness_tsv' | tail -1)"
  detail="${line#*detail=}"
  if [[ "$line" == *$'valid=1\tstate=ready'* ]]; then
    printf 'latch ready %s\n' "${detail#live platform ready }"
  else
    printf '%s\n' "$line"
  fi
}

latch_readiness_activate() {
  local mister="$1"
  local app="${MISTER_MAGIK_APP_DIR:?select a MagiK layout first}"
  local latch_rbf="$app/fpga/menu-magik-vblank-latch.rbf"

  local probe_output probe_status
  set +e
  probe_output="$(latch_readiness_probe "$mister" 2>&1)"
  probe_status="$?"
  set -e
  if [[ "$probe_status" -eq 0 ]]; then
    latch_readiness_summary "$probe_output"
    return 0
  fi
  if ! latch_readiness_is_contract_failure "$probe_output"; then
    printf '%s\n' "$probe_output" >&2
    echo "ERROR: latch readiness transport failed; refusing follow-up device calls" >&2
    return "$probe_status"
  fi

  if ! declare -F platform_manifest_verify >/dev/null 2>&1; then
    echo "ERROR: latch activation requires platform_manifest_verify" >&2
    return 2
  fi
  platform_manifest_verify "$mister" "$MISTER_MAGIK_LAYOUT" "$MISTER_MAGIK_MANIFEST" "" verify

  "$mister" agent magik launch "$latch_rbf"
  # Main re-execs through the qualified RBF. Keep this bounded and use one
  # follow-up device probe so transport failures are never retried blindly.
  sleep "${MISTER_LATCH_ACTIVATION_SETTLE_SECS:-3}"
  if ! latch_readiness_probe "$mister"; then
    echo "ERROR: verified latch activation did not become ready: $latch_rbf" >&2
    return 1
  fi
}

latch_readiness_self_test() {
  local tmp fake log
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/latch-readiness-lib.XXXXXX")"
  fake="$tmp/mister"
  log="$tmp/log"
  printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$2" >>"$LATCH_READINESS_TEST_LOG"\ncase "$2" in *latch-readiness-report*) printf "latch_readiness_tsv\\tvalid=1\\tstate=ready\\tstage=none\\treason=none\\n";; esac\n' >"$fake"
  chmod +x "$fake"
  LATCH_READINESS_TEST_LOG="$log" latch_readiness_probe "$fake"
  grep -q 'wrong-main-core' "$log"
  grep -q 'latch-readiness-report' "$log"
  latch_readiness_is_contract_failure 'latch_readiness_tsv valid=0 reason=wrong-main-core'
  if latch_readiness_is_contract_failure 'connection timed out'; then
    echo 'transport failure was misclassified as a latch contract failure' >&2
    return 1
  fi
  [[ "$(latch_readiness_summary $'header\nlatch_readiness_tsv\tvalid=1\tstate=ready\tstage=none\treason=none\tdetail=live platform ready flip_count=4 post_count=5 drop_count=0')" == \
    "latch ready flip_count=4 post_count=5 drop_count=0" ]]
  rm -rf "$tmp"
  echo "latch readiness library self-test ok"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  set -euo pipefail
  source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/magik-layout.sh"
  latch_readiness_self_test
fi
