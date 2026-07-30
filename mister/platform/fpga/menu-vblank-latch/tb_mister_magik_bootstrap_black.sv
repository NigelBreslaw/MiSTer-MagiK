// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module tb_mister_magik_bootstrap_black;

	reg  [23:0] rgb_in;
	reg         de_in;
	reg         hs_in;
	reg         vs_in;
	wire [23:0] rgb_out;
	wire        de_out;
	wire        hs_out;
	wire        vs_out;

	mister_magik_bootstrap_black dut
	(
		.rgb_in(rgb_in),
		.de_in(de_in),
		.hs_in(hs_in),
		.vs_in(vs_in),
		.rgb_out(rgb_out),
		.de_out(de_out),
		.hs_out(hs_out),
		.vs_out(vs_out)
	);

	integer rgb;
	integer timing;
	initial begin
		for (timing = 0; timing < 8; timing = timing + 1) begin
			{de_in, hs_in, vs_in} = timing[2:0];
			for (rgb = 0; rgb < 4; rgb = rgb + 1) begin
				case (rgb)
					0: rgb_in = 24'h000000;
					1: rgb_in = 24'hffffff;
					2: rgb_in = 24'hff0000;
					default: rgb_in = 24'h55aa33;
				endcase
				#1;
				if (rgb_out !== 24'h000000)
					$fatal(1, "native bootstrap pixels must be black");
				if ({de_out, hs_out, vs_out} !== {de_in, hs_in, vs_in})
					$fatal(1, "bootstrap black must preserve DE/HS/VS");
			end
		end
		$display("PASS: MagiK native bootstrap is black with timing intact");
		$finish;
	end

endmodule
