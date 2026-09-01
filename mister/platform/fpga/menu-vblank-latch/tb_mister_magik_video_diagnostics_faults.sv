// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps

module tb_mister_magik_video_diagnostics_faults;
	`include "mister_magik_video_diagnostics_protocol.svh"

	parameter integer FAULT_CASE = 0;
	reg clk_100m = 1'b0;
	reg clk_sys = 1'b0;
	reg scaler_clk = 1'b0;
	reg reset_req = 1'b1;
	reg [27:0] vbuf_address = 28'd0;
	reg [7:0] vbuf_burstcount = 8'd128;
	reg vbuf_waitrequest = 1'b0;
	reg vbuf_readdatavalid = 1'b0;
	reg vbuf_read = 1'b0;
	reg [15:0] scaler_diag_state = 16'd0;
	wire response_valid;
	wire [15:0] response_data;
	integer timeout;
	reg [15:0] expected_crc;

	function automatic [15:0] crc16_word;
		input [15:0] crc_in;
		input [15:0] word;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in;
			for(bit_index = 15; bit_index >= 0; bit_index = bit_index - 1) begin
				if(value[15] ^ word[bit_index])
					value = {value[14:0], 1'b0} ^ 16'h1021;
				else
					value = {value[14:0], 1'b0};
			end
			crc16_word = value;
		end
	endfunction

	mister_magik_scaler_fetch_liveness_state #(
		.WATCHDOG_LIMIT(24'd12),
		.RESET_QUALIFY_LIMIT(3'd4)
	) dut (
		.clk_100m(clk_100m),
		.clk_sys(clk_sys),
		.scaler_clk(scaler_clk),
		.reset_req(reset_req),
		.vbuf_address(vbuf_address),
		.vbuf_burstcount(vbuf_burstcount),
		.vbuf_waitrequest(vbuf_waitrequest),
		.vbuf_readdatavalid(vbuf_readdatavalid),
		.vbuf_read(vbuf_read),
		.scaler_diag_state(scaler_diag_state),
		.io_uio(1'b0),
		.io_strobe(1'b0),
		.io_din(16'd0),
		.response_valid(response_valid),
		.response_data(response_data)
	);

	always #5 clk_100m = ~clk_100m;
	always #7 clk_sys = ~clk_sys;
	always #3 scaler_clk = ~scaler_clk;

	task automatic accept(input [7:0] burstcount);
		begin
			@(negedge clk_100m);
			vbuf_burstcount = burstcount;
			vbuf_read = 1'b1;
			@(negedge clk_100m);
			vbuf_read = 1'b0;
			vbuf_burstcount = 8'd128;
		end
	endtask

	task automatic wait_snapshot_pending;
		begin
			timeout = 0;
			while(!dut.snapshot_pending && timeout < 80) begin
				@(posedge clk_100m);
				timeout = timeout + 1;
			end
			if(!dut.snapshot_pending)
				$fatal(1, "snapshot did not start for case %0d", FAULT_CASE);
		end
	endtask

	task automatic complete_snapshot(input bit valid_outcome);
		begin
			repeat(4) @(negedge scaler_clk);
			scaler_diag_state = 16'h0001;
			@(negedge scaler_clk);
			scaler_diag_state = valid_outcome ? 16'h0000 : 16'h0003;
			@(negedge scaler_clk);
			scaler_diag_state = 16'h0001;
			@(negedge scaler_clk);
			scaler_diag_state = 16'h0000;
		end
	endtask

	initial begin
		repeat(4) @(posedge clk_100m);
		reset_req = 1'b0;
		repeat(8) @(posedge clk_100m);

		case(FAULT_CASE)
			0: accept(8'd64);
			1: begin
				accept(8'd128);
				accept(8'd128);
				accept(8'd128);
			end
			2: begin
				@(negedge clk_100m);
				vbuf_readdatavalid = 1'b1;
				@(negedge clk_100m);
				vbuf_readdatavalid = 1'b0;
			end
			3: wait_snapshot_pending();
			4: begin
				wait_snapshot_pending();
				reset_req = 1'b1;
				repeat(3) @(posedge clk_100m);
				reset_req = 1'b0;
				complete_snapshot(1'b1);
			end
			5: begin
				wait_snapshot_pending();
				accept(8'd128);
				complete_snapshot(1'b1);
			end
			6: begin
				wait_snapshot_pending();
				complete_snapshot(1'b0);
			end
			default: $fatal(1, "unknown fault case %0d", FAULT_CASE);
		endcase

		timeout = 0;
		while(!dut.record_ready && timeout < 160) begin
			@(posedge clk_100m);
			timeout = timeout + 1;
		end
		if(!dut.record_ready)
			$fatal(1, "observer fault record did not publish for case %0d", FAULT_CASE);
		if(!dut.observer_fault)
			$fatal(1, "case %0d did not classify as observer fault", FAULT_CASE);
		if(dut.frozen_cause != FAULT_CASE[2:0])
			$fatal(1, "case %0d froze cause %0d", FAULT_CASE, dut.frozen_cause);
		if(dut.frozen_state != {
			4'd0,
			dut.avalon_terminal_fifo_depth,
			dut.avalon_terminal_return_phase,
			FAULT_CASE[2:0]
		})
			$fatal(1, "case %0d published malformed compact state %04x",
				FAULT_CASE, dut.frozen_state);
		if(dut.terminal_flags[15:8] != 8'd0 || !dut.terminal_flags[3])
			$fatal(1, "case %0d published malformed compact flags %04x",
				FAULT_CASE, dut.terminal_flags);
		expected_crc = crc16_word(
			crc16_word(MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA_CRC,
				dut.terminal_flags),
			dut.frozen_state);
		if(dut.publish_crc_work != expected_crc)
			$fatal(1, "case %0d CRC mismatch expected=%04x actual=%04x",
				FAULT_CASE, expected_crc, dut.publish_crc_work);
		$display("PASS: observer fault case %0d cause %0d", FAULT_CASE, dut.frozen_cause);
		$finish;
	end
endmodule
