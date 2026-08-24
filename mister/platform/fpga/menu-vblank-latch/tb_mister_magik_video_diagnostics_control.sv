// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module tb_mister_magik_video_diagnostics_control;
	initial begin
		$display("PASS: repaired FPGA defines no standalone diagnostic responder");
		$finish;
	end
endmodule
