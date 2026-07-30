// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module mister_magik_bootstrap_black
(
	input  wire [23:0] rgb_in,
	input  wire        de_in,
	input  wire        hs_in,
	input  wire        vs_in,
	output wire [23:0] rgb_out,
	output wire        de_out,
	output wire        hs_out,
	output wire        vs_out
);

	// Retain the native Menu pixel cone so the MagiK RBF remains directly
	// comparable with the matched stock synthesis. The kept dependency is
	// still masked to constant black before it reaches either output path.
	(* keep = "true" *) wire [23:0] native_rgb_keep;
	assign native_rgb_keep = rgb_in;
	assign rgb_out = native_rgb_keep & 24'd0;
	assign de_out = de_in;
	assign hs_out = hs_in;
	assign vs_out = vs_in;

endmodule
