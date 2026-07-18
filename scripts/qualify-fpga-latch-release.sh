#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
source "$ROOT/scripts/lib/arming-state-lib.sh"
RBF_DIR="${MISTER_FPGA_RELEASE_DIR:-$ROOT/build/fpga-vblank-latch}"
RBF="$RBF_DIR/menu-magik-vblank-latch.rbf"
META="$RBF_DIR/menu-magik-vblank-latch.metadata.txt"
REMOTE_RBF="/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf"
REMOTE_META="/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-fb"
LABEL="FPGA-LATCH-QUAL-$(date -u +%Y%m%dT%H%M%SZ)"
SOAK_SECS=0
HDMI_EVIDENCE=""
SELF_TEST=0

usage() {
  cat <<'EOF'
Usage: scripts/qualify-fpga-latch-release.sh [--label LABEL] [--soak-secs N] [--hdmi-evidence PATH] [--self-test]

Runs the bounded exact-RBF latch qualification. A commercial release uses
--soak-secs 7200 and supplies independently captured HDMI evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --label) LABEL="${2:?missing label}"; shift 2 ;;
    --soak-secs) SOAK_SECS="${2:?missing seconds}"; shift 2 ;;
    --hdmi-evidence) HDMI_EVIDENCE="${2:?missing path}"; shift 2 ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

require_probe() {
  local text="$1"
  grep -q $'fpga_latch_set_probe_tsv\tcmd=0x57\tsupported=1' <<<"$text" || return 1
  grep -q $'fpga_latch_status_tsv\tcmd=0x58\tsupported=1' <<<"$text" || return 1
  grep -q $'fpga_latch_caps_tsv\tcmd=0x59\tsupported=1' <<<"$text" || return 1
  grep -q $'protocol_version=2' <<<"$text" || return 1
  grep -q $'production_ready=1' <<<"$text" || return 1
}

require_readiness() {
  local text="$1"
  grep -Eq 'latch_readiness_tsv[[:space:]]+valid=1[[:space:]]+state=ready' <<<"$text" || return 1
  ! grep -Eq 'latch_(readiness|failure)_tsv[[:space:]]+valid=0' <<<"$text"
}

require_equal_hash() {
  [[ "$1" =~ ^[0-9a-f]{64}$ && "$1" == "$2" ]]
}

require_runtime() {
  local text="$1" expected="$2"
  grep -q "rbf_sha256=$expected" <<<"$text" || return 1
  grep -q "main_rbf=$REMOTE_RBF" <<<"$text" || return 1
  grep -q 'module_ready=1' <<<"$text" || return 1
  grep -q 'device_ready=1' <<<"$text" || return 1
}

counter_value() {
  local name="$1" text="$2"
  sed -n "s/.*${name}=\([0-9][0-9]*\).*/\1/p" <<<"$text" | tail -1
}

require_counter_advance() {
  local before="$1" after="$2" name="$3" a b
  a="$(counter_value "$name" "$before")"
  b="$(counter_value "$name" "$after")"
  [[ -n "$a" && -n "$b" && "$a" -ne "$b" ]]
}

self_test() {
  local good bad before after marker expected runtime
  good=$'fpga_latch_set_probe_tsv\tcmd=0x57\tsupported=1\nfpga_latch_status_tsv\tcmd=0x58\tsupported=1\tflip_count=4\tpost_count=5\tdrop_count=0\nfpga_latch_caps_tsv\tcmd=0x59\tsupported=1\tprotocol_version=2\tproduction_ready=1'
  bad="${good/supported=1/supported=0}"
  require_probe "$good"
  ! require_probe "$bad"
  require_readiness $'latch_readiness_tsv\tvalid=1\tstate=ready\tstage=none\treason=none'
  ! require_readiness $'latch_readiness_tsv\tvalid=0\tstate=platform-incompatible\tstage=kernel\treason=kernel-release-mismatch'
  before=$'fpga_latch_status_tsv\tflip_count=4\tpost_count=5\tdrop_count=0'
  after=$'fpga_latch_status_tsv\tflip_count=5\tpost_count=6\tdrop_count=1'
  require_counter_advance "$before" "$after" flip_count
  ! require_counter_advance "$before" "$before" flip_count
  expected="$(printf 'a%.0s' {1..64})"
  require_equal_hash "$expected" "$expected"
  ! require_equal_hash "$expected" "$(printf 'b%.0s' {1..64})"
  runtime=$'rbf_sha256='"$expected"$'\nmain_rbf=/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf\nmodule_ready=1\ndevice_ready=1'
  require_runtime "$runtime" "$expected"
  ! require_runtime "${runtime/module_ready=1/module_ready=0}" "$expected"
  marker="$(mktemp)"
  cleanup_test() { rm -f "$marker"; }
  cleanup_test
  [[ ! -e "$marker" ]]
  echo "qualification self-test valid=1 cases=hash-mismatch,unsupported-command,missing-capabilities,readiness-failure,missing-module,counter-stall,cleanup"
}

if [[ "$SELF_TEST" -eq 1 ]]; then
  self_test
  exit 0
fi

case "$SOAK_SECS" in *[!0-9]*|'') echo "--soak-secs must be a non-negative integer" >&2; exit 2;; esac
"$ROOT/scripts/checks/verify-fpga-rbf-manifest.py" "$META"
EXPECTED_HASH="$(sed -n 's/^rbf_sha256=//p' "$META")"
[[ -n "$EXPECTED_HASH" ]]
if [[ -n "$HDMI_EVIDENCE" && ! -f "$HDMI_EVIDENCE" ]]; then
  echo "missing HDMI evidence: $HDMI_EVIDENCE" >&2
  exit 1
fi

cleanup() {
  set +e
  "$MISTER" run "rm -f /tmp/mister-magik/fpga-latch-qualification.env" >/dev/null 2>&1
  arming_state_clear "$MISTER" >/dev/null 2>&1
  arming_state_assert_clean "$MISTER" >/dev/null 2>&1
  set -e
}
trap cleanup EXIT INT TERM

echo "==> Verify exact local/deployed RBF and runtime"
REMOTE_STATE="$($MISTER run "set -e; test -f '$REMOTE_META'; expected=\$(sed -n 's/^rbf_sha256=//p' '$REMOTE_META'); actual=\$(sha256sum '$REMOTE_RBF' | awk '{print \$1}'); test \"\$expected\" = '$EXPECTED_HASH'; test \"\$actual\" = '$EXPECTED_HASH'; test -e /dev/mister-magik-scanout-slots; grep -q '^mister_magik_scanout_slots ' /proc/modules; pid=\$(pidof MiSTer_MagiKDev); cmdline=\$(tr '\\000' ' ' < /proc/\$pid/cmdline); echo \"\$cmdline\" | grep -Fq '$REMOTE_RBF'; echo rbf_sha256=\$actual; echo main_rbf='$REMOTE_RBF'; echo module_ready=1; echo device_ready=1; '$REMOTE_BIN' fpga-latch-report; '$REMOTE_BIN' latch-readiness-report")"
require_runtime "$REMOTE_STATE" "$EXPECTED_HASH"
require_probe "$REMOTE_STATE"
require_readiness "$REMOTE_STATE"
BEFORE="$REMOTE_STATE"

echo "==> Deliberate over-post and recovery"
OVERFLOW="$($MISTER run "MISTER_FPGA_LATCH_PATTERN_FRAMES=12 MISTER_FPGA_LATCH_PATTERN_PERIOD_US=0 '$REMOTE_BIN' fpga-latch-pattern")"
AFTER_OVERFLOW="$($MISTER run "set -e; for i in \$(seq 1 10); do report=\$('$REMOTE_BIN' fpga-latch-report); if echo \"\$report\" | grep -q 'pending=0'; then echo \"\$report\"; exit 0; fi; sleep 1; done; echo \"\$report\"; exit 1")"
require_counter_advance "$BEFORE" "$AFTER_OVERFLOW" drop_count
RECOVERY="$($MISTER run "MISTER_FPGA_LATCH_PATTERN_FRAMES=12 MISTER_FPGA_LATCH_PATTERN_PERIOD_US=16667 '$REMOTE_BIN' fpga-latch-pattern")"
grep -q 'unsupported_posts=0' <<<"$RECOVERY"
$MISTER run "set -e; for i in \$(seq 1 10); do report=\$('$REMOTE_BIN' fpga-latch-report); if echo \"\$report\" | grep -q 'pending=0'; then exit 0; fi; sleep 1; done; echo \"\$report\"; exit 1" >/dev/null
$MISTER run "printf 'mister_magik_launch $REMOTE_RBF\\n' > /dev/MiSTer_cmd"
$MISTER run "set -e; for i in \$(seq 1 30); do pid=\$(pidof MiSTer_MagiKDev 2>/dev/null || true); if [ -n \"\$pid\" ] && tr '\\000' ' ' < /proc/\$pid/cmdline | grep -Fq '$REMOTE_RBF'; then report=\$('$REMOTE_BIN' fpga-latch-report); if echo \"\$report\" | grep -q 'pending=0' && echo \"\$report\" | grep -q 'drop_count=0'; then exit 0; fi; fi; sleep 1; done; echo \"\${report:-no latch report}\"; exit 1" >/dev/null

echo "==> Motion gates at both framebuffer geometries"
for geometry in 960x540 1280x720; do
  "$ROOT/scripts/gate-launcher-home-max-scroll-zero-drops.sh" "$LABEL-HOME-$geometry" --skip-build --ui-fb-size "$geometry"
  "$ROOT/scripts/profile-arcade-scroll.sh" "$LABEL-ARCADE-$geometry" --skip-build --skip-boot-prelude --ui-fb-size "$geometry" --frame-pacing-policy vsync-integrity
  MISTER_UI_FB_SIZE="$geometry" "$ROOT/scripts/profile-preview-scroll.sh" "$LABEL-PREVIEW-$geometry" --skip-build --visual-captures 0 --allow-visibility-misses 8
done

echo "==> Lifecycle, reload, and fallback"
"$MISTER" reboot-wait
"$ROOT/scripts/run-rust.sh" launcher 0
"$ROOT/scripts/device-launch-return-smoke.sh"
"$MISTER" run "printf 'mister_magik_launch $REMOTE_RBF\\n' > /dev/MiSTer_cmd"
"$MISTER" run "set -e; for i in \$(seq 1 30); do pid=\$(pidof MiSTer_MagiKDev 2>/dev/null || true); if [ -n \"\$pid\" ] && tr '\\000' ' ' < /proc/\$pid/cmdline | grep -Fq '$REMOTE_RBF'; then exit 0; fi; sleep 1; done; exit 1"
"$ROOT/scripts/gate-launcher-home-max-scroll-zero-drops.sh" "$LABEL-FB0" --skip-build --present-backend fb0-dirty
MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden "$ROOT/scripts/run-rust.sh" launcher 0

if [[ "$SOAK_SECS" -gt 0 ]]; then
  echo "==> Bounded latch-motion soak (${SOAK_SECS}s)"
  "$ROOT/scripts/gate-launcher-home-max-scroll-zero-drops.sh" "$LABEL-SOAK" --skip-build --secs "$SOAK_SECS" --ui-fb-size 960x540
fi

FINAL="$($MISTER run "'$REMOTE_BIN' fpga-latch-report; '$REMOTE_BIN' latch-readiness-report")"
require_probe "$FINAL"
require_readiness "$FINAL"
require_counter_advance "$BEFORE" "$FINAL" flip_count
printf 'fpga_latch_qualification_tsv\tlabel=%s\trbf_sha256=%s\tsoak_secs=%s\thdmi_evidence=%s\tvalid=1\n' \
  "$LABEL" "$EXPECTED_HASH" "$SOAK_SECS" "${HDMI_EVIDENCE:-not-supplied}"
