// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module mister_magik_video_diagnostics_avalon (
	input  wire         clk_100m,
	input  wire         monitor_armed_async,
	input  wire         snapshot_request_toggle_async,
	input  wire [15:0]  diagnostic_generation_async,
	input  wire         route_context_toggle_async,
	input  wire [31:0]  expected_base_async,
	input  wire [31:0]  expected_slot_end_async,
	input  wire [15:0]  expected_route_epoch_async,
	input  wire [15:0]  expected_route_flags_async,
	input  wire         frame_marker_async,
	input  wire         reset_out_async,
	input  wire [27:0]  vbuf_address,
	input  wire [7:0]   vbuf_burstcount,
	input  wire         vbuf_waitrequest,
	input  wire         vbuf_readdatavalid,
	input  wire         vbuf_read,
	input  wire         vbuf_write,
	input  wire [15:0]  vbuf_byteenable,
	output reg          fault_toggle = 1'b0,
	output reg  [7:0]   fault_trigger = 8'd0,
	output reg          snapshot_ack_toggle = 1'b0,
	output wire [239:0] snapshot_payload
);

`include "mister_magik_video_diagnostics_protocol.svh"

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg armed_meta = 1'b0, armed = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg request_meta = 1'b0, request_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg route_meta = 1'b0, route_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg frame_meta = 1'b0, frame_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg reset_meta = 1'b0, reset_sync = 1'b0;
	reg request_seen = 1'b0, route_seen = 1'b0, frame_seen = 1'b0;
	reg request_capture_pending = 1'b0, request_ack_pending = 1'b0;
	reg route_capture_pending = 1'b0, fault_notify_pending = 1'b0;
	reg [15:0] snapshot_generation = 16'd0;

	reg [31:0] expected_base = 32'd0, expected_slot_end = 32'd0;
	reg [15:0] route_epoch = 16'd0;
	reg [4:0] route_flags = 5'd0;
	reg [15:0] accepted_bursts = 16'd0, returned_beats = 16'd0;
	reg [20:0] outstanding = 21'd0, stall_age = 21'd0;
	reg [27:0] first_address = 28'd0, last_address = 28'd0;
	reg [9:0] fault_flags = 10'd0;
	reg [1:0] no_read_frames = 2'd0;
	reg reads_since_frame = 1'b0;
	reg frozen = 1'b0, mailbox_overrun = 1'b0;
	reg freeze_request_now;
	reg [7:0] freeze_request_trigger;
	reg [9:0] freeze_request_flags;
	reg [9:0] observed_flags_now;
	wire route_context_changing =
		(route_sync != route_seen) || route_capture_pending ||
		({expected_base, expected_slot_end, route_epoch, route_flags} !=
		 {expected_base_async, expected_slot_end_async,
		  expected_route_epoch_async, expected_route_flags_async[4:0]});

	wire [15:0] snapshot_state_word =
		(frozen ? (MAGIK_VIDEO_DIAGNOSTICS_STATE_FROZEN |
			MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_MONITOR_ARMED |
			MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_SNAPSHOT_FROZEN |
			(mailbox_overrun ? MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_MAILBOX_OVERRUN : 16'd0)) :
		 (armed ? (MAGIK_VIDEO_DIAGNOSTICS_STATE_ARMED |
			MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_MONITOR_ARMED) :
			MAGIK_VIDEO_DIAGNOSTICS_STATE_IDLE));
	assign snapshot_payload = {
		{6'd0, fault_flags}, returned_beats, accepted_bursts,
		{4'd0, last_address[27:16]}, last_address[15:0],
		{4'd0, first_address[27:16]}, first_address[15:0],
		expected_base[31:16], expected_base[15:0],
		{11'd0, route_flags}, route_epoch, snapshot_generation, {8'd0, fault_trigger},
		snapshot_state_word, MAGIK_VIDEO_DIAGNOSTICS_SCHEMA};

	task automatic request_freeze;
		input [7:0] new_trigger;
		input [9:0] new_flags;
		begin
			if(armed && !frozen && !freeze_request_now) begin
				freeze_request_now = 1'b1;
				freeze_request_trigger = new_trigger;
				freeze_request_flags = new_flags;
			end
		end
	endtask

	always @(posedge clk_100m) begin
		freeze_request_now = 1'b0;
		freeze_request_trigger = MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_NONE;
		freeze_request_flags = 10'd0;
		observed_flags_now = 10'd0;
		armed_meta <= monitor_armed_async;
		armed <= armed_meta;
		request_meta <= snapshot_request_toggle_async;
		request_sync <= request_meta;
		route_meta <= route_context_toggle_async;
		route_sync <= route_meta;
		frame_meta <= frame_marker_async;
		frame_sync <= frame_meta;
		reset_meta <= reset_out_async;
		reset_sync <= reset_meta;
		if(fault_notify_pending) begin
			fault_notify_pending <= 1'b0;
			fault_toggle <= ~fault_toggle;
		end
		if(request_ack_pending) begin
			request_ack_pending <= 1'b0;
			snapshot_ack_toggle <= ~snapshot_ack_toggle;
		end

		if(!frozen && ((route_sync != route_seen) ||
		   (!route_capture_pending &&
		    ({expected_base, expected_slot_end, route_epoch, route_flags} !=
		     {expected_base_async, expected_slot_end_async,
		      expected_route_epoch_async, expected_route_flags_async[4:0]})))) begin
			if(route_capture_pending) mailbox_overrun <= 1'b1;
			route_seen <= route_sync;
			expected_base <= expected_base_async;
			expected_slot_end <= expected_slot_end_async;
			route_epoch <= expected_route_epoch_async;
			route_flags <= expected_route_flags_async[4:0];
			route_capture_pending <= 1'b1;
		end
		else if(!frozen && route_capture_pending) begin
			if({expected_base, expected_slot_end, route_epoch, route_flags} ==
			   {expected_base_async, expected_slot_end_async,
				expected_route_epoch_async, expected_route_flags_async[4:0]})
				route_capture_pending <= 1'b0;
			else begin
				expected_base <= expected_base_async;
				expected_slot_end <= expected_slot_end_async;
				route_epoch <= expected_route_epoch_async;
				route_flags <= expected_route_flags_async[4:0];
			end
		end

		frame_seen <= frame_sync;
		if(frame_sync && !frame_seen && !frozen) begin
			if(armed) begin
				if(reads_since_frame) no_read_frames <= 2'd0;
				else if(no_read_frames < 3) no_read_frames <= no_read_frames + 1'd1;
				if(!reads_since_frame && no_read_frames == 1)
					request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_AVALON_NO_READS,
						MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_NO_READS);
				reads_since_frame <= 1'b0;
			end
		end

		if(vbuf_read && !vbuf_waitrequest && !frozen) begin
			if(accepted_bursts == 0) first_address <= vbuf_address;
			last_address <= vbuf_address;
			reads_since_frame <= 1'b1;
			if(accepted_bursts != 16'hffff) accepted_bursts <= accepted_bursts + 1'd1;
			if(outstanding <= 21'h1fff7f) outstanding <= outstanding + vbuf_burstcount;
			else request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_AVALON_RETURN,
				MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_COUNTER_OVERFLOW);
			if(vbuf_burstcount != 8'd128) begin
				request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_AVALON_BURST,
					MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_BAD_BURSTCOUNT);
			end
			if(!route_context_changing && (({vbuf_address,4'd0} < expected_base) ||
			   ({vbuf_address,4'd0} >= expected_slot_end)))
				request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_AVALON_ADDRESS,
					MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_ADDRESS_OUTSIDE_SLOT);
		end

		if(vbuf_readdatavalid && !frozen) begin
			if(returned_beats != 16'hffff) returned_beats <= returned_beats + 1'd1;
			if((outstanding == 0) && !(vbuf_read && !vbuf_waitrequest))
				request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_AVALON_RETURN,
					MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_UNEXPECTED_RETURN);
			else if(vbuf_read && !vbuf_waitrequest)
				outstanding <= outstanding + vbuf_burstcount - 1'd1;
			else outstanding <= outstanding - 1'd1;
		end

		if(!frozen) begin
			if((outstanding != 0) || (vbuf_read && vbuf_waitrequest)) begin
				if(stall_age < 21'h100000) stall_age <= stall_age + 1'd1;
				if(stall_age == 21'h0fffff)
					request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_AVALON_TIMEOUT,
						MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_REQUEST_TIMEOUT);
			end
			else stall_age <= 21'd0;
		end

		if(reset_sync && (outstanding != 0) && !frozen)
			request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_AVALON_RETURN,
				MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_RESET_OUTSTANDING);

		if(vbuf_write && !vbuf_waitrequest && !frozen) begin
			observed_flags_now = observed_flags_now |
				MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_WRITE_SEEN[9:0] |
				((vbuf_byteenable != 16'hffff) ?
				 MAGIK_VIDEO_DIAGNOSTICS_AVALON_FAULT_FLAGS_BAD_BYTEENABLE[9:0] : 10'd0);
		end

		if(request_sync != request_seen) begin
			request_seen <= request_sync;
			snapshot_generation <= diagnostic_generation_async;
			request_capture_pending <= 1'b1;
		end
		else if(request_capture_pending) begin
			if(snapshot_generation == diagnostic_generation_async) begin
				request_capture_pending <= 1'b0;
				frozen <= 1'b1;
				request_ack_pending <= 1'b1;
			end
			else snapshot_generation <= diagnostic_generation_async;
		end

		if(freeze_request_now) begin
			frozen <= 1'b1;
			fault_trigger <= freeze_request_trigger;
			fault_flags <= fault_flags | freeze_request_flags | observed_flags_now;
			fault_notify_pending <= 1'b1;
		end
		else if(observed_flags_now != 0)
			fault_flags <= fault_flags | observed_flags_now;
	end

endmodule

`default_nettype wire
