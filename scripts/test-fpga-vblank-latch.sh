#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rtl_dir="$repo_root/experiments/fpga-vblank-latch"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-vblank-latch.XXXXXX")"

cleanup() {
	rm -rf "$build_dir"
}
trap cleanup EXIT

iverilog -g2012 -Wall -Wimplicit \
	-s tb_mister_magik_vblank_latch \
	-o "$build_dir/tb_mister_magik_vblank_latch.vvp" \
	"$rtl_dir/mister_magik_vblank_latch.sv" \
	"$rtl_dir/tb_mister_magik_vblank_latch.sv"

vvp "$build_dir/tb_mister_magik_vblank_latch.vvp"
