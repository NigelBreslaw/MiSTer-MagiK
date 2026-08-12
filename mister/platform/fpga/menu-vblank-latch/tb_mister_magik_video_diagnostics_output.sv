// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_output;
`include "mister_magik_video_diagnostics_protocol.svh"
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
	reg commit_request = 1'b0, no_request = 1'b0;
	reg [15:0] commit_generation = 16'h3101, no_generation = 16'h4201;
	wire commit_heartbeat, commit_fault, commit_ack;
	wire no_heartbeat, no_fault, no_ack;
	wire [7:0] commit_trigger, no_trigger;
	wire [239:0] commit_payload, no_payload;

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
		.hdmi_pll_locked_async(pll_locked), .hdmi_out_d(data),
		.hdmi_out_de(de), .hdmi_out_hs(hs), .hdmi_out_vs(vs),
		.heartbeat_toggle(heartbeat), .fault_toggle(fault),
		.fault_trigger(trigger), .snapshot_ack_toggle(ack),
		.snapshot_payload(payload)
	);

	mister_magik_video_diagnostics_output commit_dut (
		.hdmi_tx_clk(clk), .monitor_armed_async(armed),
		.snapshot_request_toggle_async(commit_request),
		.diagnostic_generation_async(commit_generation),
		.route_context_toggle_async(route_toggle),
		.expected_route_epoch_async(route_epoch),
		.expected_active_seq_async(active_seq),
		.expected_route_flags_async(route_flags),
		.mux_direct_async(mux_direct), .mux_csync_async(mux_csync),
		.reset_req_async(reset_req), .cfg_done_async(cfg_done),
		.hdmi_pll_locked_async(pll_locked), .hdmi_out_d(data),
		.hdmi_out_de(de), .hdmi_out_hs(hs), .hdmi_out_vs(vs),
		.heartbeat_toggle(commit_heartbeat), .fault_toggle(commit_fault),
		.fault_trigger(commit_trigger), .snapshot_ack_toggle(commit_ack),
		.snapshot_payload(commit_payload)
	);

	mister_magik_video_diagnostics_output no_request_dut (
		.hdmi_tx_clk(clk), .monitor_armed_async(armed),
		.snapshot_request_toggle_async(no_request),
		.diagnostic_generation_async(no_generation),
		.route_context_toggle_async(route_toggle),
		.expected_route_epoch_async(route_epoch),
		.expected_active_seq_async(active_seq),
		.expected_route_flags_async(route_flags),
		.mux_direct_async(mux_direct), .mux_csync_async(mux_csync),
		.reset_req_async(reset_req), .cfg_done_async(cfg_done),
		.hdmi_pll_locked_async(pll_locked), .hdmi_out_d(data),
		.hdmi_out_de(de), .hdmi_out_hs(hs), .hdmi_out_vs(vs),
		.heartbeat_toggle(no_heartbeat), .fault_toggle(no_fault),
		.fault_trigger(no_trigger), .snapshot_ack_toggle(no_ack),
		.snapshot_payload(no_payload)
	);

	function automatic [15:0] word_at;
		input integer index;
		begin word_at = payload[index*16 +: 16]; end
	endfunction

	function automatic [15:0] commit_word_at;
		input integer index;
		begin commit_word_at = commit_payload[index*16 +: 16]; end
	endfunction

	function automatic [15:0] no_word_at;
		input integer index;
		begin no_word_at = no_payload[index*16 +: 16]; end
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

	reg first_fault, fault_before;
	reg [239:0] frozen_payload;
	reg [15:0] frozen_route_epoch, frozen_active_seq;
	reg [7:0] pending_flags;
	reg [2:0] pending_source_flags;
	reg [4:0] pending_control_flags;
	reg [239:0] repeated_request_payload;
	integer word_index;
	initial begin
		repeat(4) @(negedge clk);
		route_toggle = ~route_toggle;
		armed = 1'b1;
		// Present a stale completed frame on the exact clock that arming and VS
		// become visible. The arm boundary must discard it rather than enqueue a
		// native fault or preserve pre-arm source stability.
		repeat(2) @(negedge clk);
		dut.have_frame = 1'b1;
		dut.reference_valid = 1'b1;
		dut.consecutive_black = 2'd1;
		dut.saw_de = 1'b0;
		dut.source_stable = 1'b1;
		vs = 1'b1;
		@(negedge clk); vs = 1'b0;
		repeat(2) @(negedge clk);
		if(dut.native_fault_pending || fault || trigger != 0 ||
		   dut.reference_valid || dut.source_stable)
			$fatal(1, "arm/VS boundary retained a stale completed frame");
		// Recover the newest route even when an even number of toggles was
		// invisible while the output clock was stopped.
		route_epoch = route_epoch + 2'd2;
		active_seq = active_seq + 2'd2;
		repeat(4) @(negedge clk);
		if(dut.route_epoch != route_epoch || dut.active_sequence != active_seq)
			$fatal(1, "coalesced route epoch did not recover latest output context");

		// Two identical colored frames establish a reference; white blanking is ignored.
		drive_frame(24'h204080);
		drive_frame(24'h204080);
		drive_frame(24'h204080);
		if(fault) $fatal(1, "colored reference faulted");

		// Two complete frames with no DE must still diagnose a black output.
		drive_frame_without_de();
		drive_frame_without_de();

		// Make a manual request ready on the native commit edge and overflow the
		// period counter on the selection edge. Black and timing are both true;
		// black must retain first-fault priority and the exact completed-frame
		// evidence must be immutable while the selected record is pending.
		generation = 16'h2001;
		request = ~request;
		@(negedge clk);
		commit_request = ~commit_request;
		@(negedge clk);
		dut.frame_period = 24'hffffff;
		fault_before = fault;
		vs = 1'b1;
		@(negedge clk); vs = 1'b0;
		if(!dut.native_fault_pending || dut.frozen || trigger != 0 ||
		   fault != fault_before || ack == request)
			$fatal(1, "native selected fault did not wait for serialized commit");
		if(!dut.request_capture_pending)
			$fatal(1, "coincident manual request was consumed before native commit");
		if(word_at(3) != 16'h2001)
			$fatal(1, "generation was not sampled on request recognition at native enqueue");
		generation = 16'h2002;
		pending_flags = dut.native_fault_flags;
		pending_source_flags = dut.snapshot_source_flags;
		pending_control_flags = dut.snapshot_control_flags;
		frozen_route_epoch = dut.route_epoch;
		frozen_active_seq = dut.active_sequence;
		route_epoch = route_epoch + 1'd1;
		active_seq = active_seq + 1'd1;
		route_flags = 16'h0011;
		route_toggle = ~route_toggle;
		mux_direct = 1'b1;
		cfg_done = 1'b0;
		pll_locked = 1'b0;
		@(negedge clk);
		if(dut.native_fault_pending || !dut.frozen || trigger != 8'd10)
			$fatal(1, "native selected fault did not commit first");
		if(fault != fault_before || ack == request)
			$fatal(1, "native evidence toggled before one stable commit clock");
		if(dut.native_fault_flags != pending_flags ||
		   dut.snapshot_source_flags != pending_source_flags ||
		   dut.snapshot_control_flags != pending_control_flags)
			$fatal(1, "pending selected-frame evidence changed before commit");
		if(dut.route_epoch != frozen_route_epoch ||
		   dut.active_sequence != frozen_active_seq)
			$fatal(1, "route context advanced while native evidence was pending");
		if(word_at(3) != 16'h2001)
			$fatal(1, "recognized generation changed before native commit");
		if(!commit_dut.frozen || !commit_dut.request_capture_pending ||
		   commit_ack == commit_request || commit_word_at(3) != 16'h3101)
			$fatal(1, "request recognized at native commit was consumed early");
		if(no_word_at(3) != 0 || no_ack != no_request)
			$fatal(1, "native commit without a request changed generation or ack");
		commit_generation = 16'h3102;
		frozen_payload = payload;
		@(negedge clk);
		if(trigger != 8'd10) $fatal(1, "black output was not classified");
		if(fault == fault_before || ack != request)
			$fatal(1, "serialized fault and manual acknowledgement latency mismatch");
		if((word_at(14) & 16'h00a8) != 16'h0088 ||
		   (word_at(14) & 16'h0020) != 0)
			$fatal(1, "black priority or selection-edge overflow evidence mismatch");
		if((word_at(14) & 16'h0700) != 0)
			$fatal(1, "lower-priority timing geometry escaped black selection");
		if(word_at(12) != 16'hffff || word_at(13) != 16'h00ff)
			$fatal(1, "fault period did not retain the selected completed frame");
		if(payload !== frozen_payload)
			$fatal(1, "payload changed between commit and notification");
		if(commit_dut.request_capture_pending || commit_ack == commit_request ||
		   commit_word_at(3) != 16'h3101)
			$fatal(1, "recognized-at-commit request did not defer exactly one consume cycle");
		@(negedge clk);
		if(commit_ack != commit_request || commit_word_at(3) != 16'h3101)
			$fatal(1, "recognized-at-commit acknowledgement or generation was unstable");
		first_fault = fault;

		// Frozen evidence is immutable even when the mux subsequently changes.
		drive_frame(24'hffffff);
		if(fault != first_fault || trigger != 8'd10) $fatal(1, "first output fault changed");

		if(word_at(0) != MAGIK_VIDEO_DIAGNOSTICS_SCHEMA ||
		   word_at(2) != 16'd10 || word_at(3) != 16'h2001)
			$fatal(1, "output snapshot identity mismatch");
		if(word_at(4) != frozen_route_epoch || word_at(5) != frozen_active_seq)
			$fatal(1, "output route context mismatch");
		if((word_at(14) & 16'h0008) == 0 || (word_at(14) & 16'h0001) != 0)
			$fatal(1, "no-DE black frame evidence was not distinguished");

		// A later request against already-frozen native evidence may update only
		// the generation word. Recognition captures the bundled value; consuming
		// the request and acknowledging it must not resample a changed async bus.
		repeated_request_payload = no_payload;
		no_generation = 16'h4202;
		no_request = ~no_request;
		repeat(3) @(negedge clk);
		if(no_word_at(3) != 16'h4202 || !no_request_dut.request_capture_pending ||
		   no_ack == no_request)
			$fatal(1, "frozen repeated request was not sampled on recognition");
		for(word_index = 0; word_index < 15; word_index = word_index + 1)
			if(word_index != 3 && no_word_at(word_index) !==
			   repeated_request_payload[word_index*16 +: 16])
				$fatal(1, "repeated request mutated frozen evidence word %0d", word_index);
		no_generation = 16'h4203;
		@(negedge clk);
		if(no_request_dut.request_capture_pending || no_ack == no_request ||
		   no_word_at(3) != 16'h4202)
			$fatal(1, "frozen repeated request did not consume captured generation");
		@(negedge clk);
		if(no_ack != no_request || no_word_at(3) != 16'h4202)
			$fatal(1, "frozen repeated request acknowledgement was not stable");
		repeat(2) @(negedge clk);
		if(no_ack != no_request || no_word_at(3) != 16'h4202)
			$fatal(1, "frozen repeated request acknowledgement toggled twice");
		frozen_payload = payload;
		mux_csync = 1'b1;
		drive_frame(24'hffffff);
		if(payload !== frozen_payload) $fatal(1, "acknowledged output mailbox changed");
		$display("video diagnostics output tests passed");
		$finish;
	end
endmodule

`default_nettype wire
