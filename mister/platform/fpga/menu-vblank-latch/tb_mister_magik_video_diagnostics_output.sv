// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_output;
	reg clk = 1'b0;
	always #4 clk = ~clk;
	reg armed = 1'b0, request = 1'b0, route_toggle = 1'b0;
	reg [15:0] generation = 16'd4, route_epoch = 16'd9, active_seq = 16'd12;
	reg [15:0] route_flags = 16'h0005;
	reg mux_direct = 1'b0, mux_csync = 1'b0;
	reg reset_req = 1'b0, cfg_done = 1'b1, pll_locked = 1'b1;
	reg [23:0] data = 24'd0;
	reg de = 1'b0, hs = 1'b0, vs = 1'b0;
	wire heartbeat, fault, ack;
	wire [7:0] trigger;
	wire [239:0] payload;

	mister_magik_video_diagnostics_output dut (
		.hdmi_tx_clk(clk), .monitor_armed_async(armed),
		.snapshot_request_toggle_async(request),
		.diagnostic_generation_async(generation),
		.route_context_toggle_async(route_toggle),
		.expected_route_epoch_async(route_epoch),
		.expected_active_seq_async(active_seq),
		.expected_route_flags_async(route_flags),
		.mux_direct_async(mux_direct), .mux_csync_async(mux_csync),
		.reset_req_async(reset_req), .cfg_done_async(cfg_done),
		.pll_adjust_locked_async(pll_locked), .hdmi_out_d(data),
		.hdmi_out_de(de), .hdmi_out_hs(hs), .hdmi_out_vs(vs),
		.heartbeat_toggle(heartbeat), .fault_toggle(fault),
		.fault_trigger(trigger), .snapshot_ack_toggle(ack),
		.snapshot_payload(payload)
	);

	function automatic [15:0] word_at;
		input integer index;
		begin word_at = payload[index*16 +: 16]; end
	endfunction

	task automatic vs_pulse;
		begin
			@(negedge clk); vs = 1'b1;
			@(negedge clk); vs = 1'b0;
		end
	endtask

	task automatic drive_frame;
		input [23:0] color;
		integer line, pixel;
		begin
			vs_pulse();
			for(line = 0; line < 2; line = line + 1) begin
				@(negedge clk); hs = 1'b1; data = 24'hffffff;
				@(negedge clk); hs = 1'b0;
				for(pixel = 0; pixel < 4; pixel = pixel + 1) begin
					de = 1'b1; data = color;
					@(negedge clk);
				end
				de = 1'b0; data = 24'hffffff;
				repeat(2) @(negedge clk);
			end
		end
	endtask

	task automatic drive_frame_without_de;
		integer line;
		begin
			vs_pulse();
			for(line = 0; line < 2; line = line + 1) begin
				@(negedge clk); hs = 1'b1; data = 24'h000000;
				@(negedge clk); hs = 1'b0;
				repeat(6) @(negedge clk);
			end
		end
	endtask

	reg first_fault;
	reg [239:0] frozen_payload;
	initial begin
		repeat(4) @(negedge clk);
		route_toggle = ~route_toggle;
		armed = 1'b1;
		repeat(4) @(negedge clk);

		// Two identical colored frames establish a reference; white blanking is ignored.
		drive_frame(24'h204080);
		drive_frame(24'h204080);
		drive_frame(24'h204080);
		if(fault) $fatal(1, "colored reference faulted");

		// Two complete frames with no DE must still diagnose a black output.
		drive_frame_without_de();
		drive_frame_without_de();
		vs_pulse();
		repeat(3) @(negedge clk);
		if(trigger != 8'd10) $fatal(1, "black output was not classified");
		first_fault = fault;

		// Frozen evidence is immutable even when the mux subsequently changes.
		mux_direct = 1'b1;
		drive_frame(24'hffffff);
		if(fault != first_fault || trigger != 8'd10) $fatal(1, "first output fault changed");

		request = ~request;
		repeat(5) @(negedge clk);
		if(ack != request) $fatal(1, "output snapshot did not acknowledge");
		if(word_at(0) != 16'd2 || word_at(2) != 16'd10 || word_at(3) != generation)
			$fatal(1, "output snapshot identity mismatch");
		if(word_at(4) != route_epoch || word_at(5) != active_seq)
			$fatal(1, "output route context mismatch");
		if((word_at(14) & 16'h0008) == 0 || (word_at(14) & 16'h0001) != 0)
			$fatal(1, "no-DE black frame evidence was not distinguished");
		frozen_payload = payload;
		mux_csync = 1'b1;
		drive_frame(24'hffffff);
		if(payload !== frozen_payload) $fatal(1, "acknowledged output mailbox changed");
		$display("video diagnostics output tests passed");
		$finish;
	end
endmodule

`default_nettype wire
