// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

module mister_magik_scaler_copy_tail_formal;
	reg formal_clk = 1'b0;
	reg past_valid = 1'b0;
	reg retired = 1'b0;

	(* anyseq *) wire reset_n;
	(* anyseq *) wire start;
	(* anyseq *) wire [2:0] format;
	(* anyseq *) wire [3:0] phase;
	wire active;
	wire shift;
	wire pixel_valid;
	wire terminal;
	wire done;
	wire [4:0] age;
	wire nonlast_terminal;
	wire early_last_terminal;

	mister_magik_scaler_copy_tail_formal_dut dut (
		.clk(formal_clk),
		.reset_n(reset_n),
		.start(start),
		.format(format),
		.phase(phase),
		.active_o(active),
		.shift_o(shift),
		.pixel_valid_o(pixel_valid),
		.terminal_o(terminal),
		.done_o(done),
		.age_o(age),
		.nonlast_terminal_o(nonlast_terminal),
		.early_last_terminal_o(early_last_terminal)
	);

	always @($global_clock) begin
		formal_clk <= !formal_clk;
		past_valid <= 1'b1;
		if (!past_valid)
			assume(!reset_n);
		assume(format == 3'b011 || format == 3'b100 ||
		       format == 3'b101 || format == 3'b110);

		assert(nonlast_terminal);
		assert(!early_last_terminal);
		if (active) begin
			assert(shift);
			assert(!pixel_valid);
			assert(age < 18);
			if (age == 17)
				assert(terminal);
		end
		if (done)
			retired <= 1'b1;
		cover(retired);
	end
endmodule
