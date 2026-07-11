#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RTL="$ROOT/experiments/fpga-vblank-latch/mister_magik_scanout_mailbox.sv"
TB="$ROOT/experiments/fpga-vblank-latch/tb_mister_magik_scanout_mailbox.sv"
OUT="$(mktemp -d "${TMPDIR:-/tmp}/magik-mailbox-test.XXXXXX")"
trap 'rm -rf "$OUT"' EXIT

if ! command -v iverilog >/dev/null 2>&1 || ! command -v vvp >/dev/null 2>&1; then
  echo "Icarus Verilog is required (brew install icarus-verilog)" >&2
  exit 1
fi

iverilog -g2012 -Wall -s tb_mister_magik_scanout_mailbox \
  -o "$OUT/mailbox.vvp" "$RTL" "$TB"
vvp "$OUT/mailbox.vvp"

# Elaborate the production bridge wrapper against the signature-only hard-IP
# stand-in in the testbench. Quartus supplies the real primitive in RBF builds.
iverilog -g2012 -Wall -s mister_magik_scanout_mailbox_bridge \
  -o "$OUT/bridge.vvp" "$RTL" "$TB"
echo "PASS: Cyclone V bridge wrapper elaborates"
