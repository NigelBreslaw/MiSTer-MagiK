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
	-s tb_mister_magik_video_diagnostics_control \
	-o "$build_dir/tb_mister_magik_video_diagnostics_control.vvp" \
	"$rtl_dir/mister_magik_video_diagnostics_control.sv" \
	"$rtl_dir/tb_mister_magik_video_diagnostics_control.sv"

vvp "$build_dir/tb_mister_magik_video_diagnostics_control.vvp"

iverilog -g2012 -Wall -Wimplicit -I "$rtl_dir" \
	-s tb_mister_magik_scaler_completion_cdc \
	-o "$build_dir/tb_mister_magik_scaler_completion_cdc.vvp" \
	"$rtl_dir/mister_magik_video_diagnostics_avalon.sv" \
	"$rtl_dir/tb_mister_magik_scaler_completion_cdc.sv"

vvp "$build_dir/tb_mister_magik_scaler_completion_cdc.vvp"

iverilog -g2012 -Wall -Wimplicit \
	-s tb_mister_magik_bootstrap_black \
	-o "$build_dir/tb_mister_magik_bootstrap_black.vvp" \
	"$rtl_dir/mister_magik_bootstrap_black.sv" \
	"$rtl_dir/tb_mister_magik_bootstrap_black.sv"

vvp "$build_dir/tb_mister_magik_bootstrap_black.vvp"

grep -Fq "assign rgb_out = 24'd0;" \
	"$rtl_dir/mister_magik_bootstrap_black.sv"
! grep -Fq "native_rgb_keep" "$rtl_dir/mister_magik_bootstrap_black.sv"
! grep -Fq "_unused" "$rtl_dir/mister_magik_bootstrap_black.sv"
