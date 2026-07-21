#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
rtl_dir="$repo_root/mister/platform/fpga/menu-vblank-latch"
build_dir="$(mktemp -d "${TMPDIR:-/tmp}/mister-magik-vblank-latch.XXXXXX")"

cleanup() {
	rm -rf "$build_dir"
}
trap cleanup EXIT

iverilog -g2012 -Wall -Wimplicit -I "$rtl_dir" \
	-s tb_mister_magik_vblank_latch \
	-o "$build_dir/tb_mister_magik_vblank_latch.vvp" \
	"$rtl_dir/mister_magik_vblank_latch.sv" \
	"$rtl_dir/tb_mister_magik_vblank_latch.sv"

vvp "$build_dir/tb_mister_magik_vblank_latch.vvp"

iverilog -g2012 -Wall -Wimplicit -I "$rtl_dir" \
	-s tb_mister_magik_crt_timing \
	-o "$build_dir/tb_mister_magik_crt_timing.vvp" \
	"$rtl_dir/mister_magik_crt_timing.sv" \
	"$rtl_dir/tb_mister_magik_crt_timing.sv"

vvp "$build_dir/tb_mister_magik_crt_timing.vvp"

iverilog -g2012 -Wall -Wimplicit -I "$rtl_dir" \
	-s tb_mister_magik_crt_reader \
	-o "$build_dir/tb_mister_magik_crt_reader.vvp" \
	"$rtl_dir/mister_magik_crt_reader.sv" \
	"$rtl_dir/tb_mister_magik_crt_reader.sv"

vvp "$build_dir/tb_mister_magik_crt_reader.vvp"
