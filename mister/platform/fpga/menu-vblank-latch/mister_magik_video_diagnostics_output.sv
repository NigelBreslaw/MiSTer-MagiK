// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Passive observer of the final registered HDMI signals. No output from this
// module is connected to the video, clock, reset, or route datapaths.
module mister_magik_video_diagnostics_output (
	input  wire         hdmi_tx_clk,
	input  wire         monitor_armed_async,
	input  wire         snapshot_request_toggle_async,
	input  wire [15:0]  diagnostic_generation_async,
	input  wire         route_context_toggle_async,
	input  wire [15:0]  expected_route_epoch_async,
	input  wire [15:0]  expected_active_seq_async,
	input  wire [15:0]  expected_route_flags_async,
	input  wire         mux_direct_async,
	input  wire         mux_csync_async,
	input  wire         reset_req_async,
	input  wire         cfg_done_async,
	input  wire         pll_adjust_locked_async,
	input  wire [23:0]  hdmi_out_d,
	input  wire         hdmi_out_de,
	input  wire         hdmi_out_hs,
	input  wire         hdmi_out_vs,
	output reg          heartbeat_toggle = 1'b0,
	output reg          fault_toggle = 1'b0,
	output reg  [7:0]   fault_trigger = 8'd0,
	output reg          snapshot_ack_toggle = 1'b0,
	output wire [495:0] snapshot_payload
);

`include "mister_magik_video_diagnostics_protocol.svh"

	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg armed_meta = 1'b0, armed = 1'b0;
	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg request_meta = 1'b0, request_sync = 1'b0;
	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg route_meta = 1'b0, route_sync = 1'b0;
	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg direct_meta = 1'b0, direct_sync = 1'b0;
	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg csync_meta = 1'b0, csync_sync = 1'b0;
	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg reset_meta = 1'b0, reset_sync = 1'b0;
	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg cfg_meta = 1'b0, cfg_sync = 1'b0;
	(* ASYNC_REG = "TRUE", altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg pll_meta = 1'b0, pll_sync = 1'b0;

	reg armed_d = 1'b0, request_seen = 1'b0, route_seen = 1'b0;
	reg request_capture_pending = 1'b0, request_ack_pending = 1'b0;
	reg route_capture_pending = 1'b0, fault_notify_pending = 1'b0;
	reg [15:0] snapshot_generation = 16'd0;
	reg vs_d = 1'b0, hs_d = 1'b0, de_d = 1'b0;
	reg [15:0] route_epoch = 16'd0, active_sequence = 16'd0, route_flags = 16'd0;
	reg [31:0] frame_period = 32'd0, active_pixels = 32'd0;
	reg [15:0] line_count = 16'd0, active_lines = 16'd0;
	reg saw_de = 1'b0, saw_nonblack = 1'b0, saw_nonwhite = 1'b0;
	reg have_frame = 1'b0, reference_valid = 1'b0;
	reg [31:0] reference_period = 32'd0, reference_pixels = 32'd0;
	reg [15:0] reference_lines = 16'd0, reference_active_lines = 16'd0;
	reg [15:0] reference_flags = 16'd0, reference_source_flags = 16'd0;
	reg [15:0] last_frame_source = 16'd0;
	reg source_stable = 1'b0;
	reg [31:0] fault_period = 32'd0, fault_pixels = 32'd0;
	reg [15:0] fault_lines = 16'd0, fault_active_lines = 16'd0;
	reg [15:0] fault_flags = 16'd0, geometry_faults = 16'd0;
	reg [15:0] control_changes = 16'd0;
	reg [1:0] consecutive_black = 2'd0, consecutive_white = 2'd0;
	reg [1:0] consecutive_timing = 2'd0;
	reg [31:0] frame_count = 32'd0;
	reg [15:0] snapshot_source_flags = 16'd0, snapshot_control_flags = 16'd0;
	reg snapshot_heartbeat = 1'b0;
	reg frozen = 1'b0, mailbox_overrun = 1'b0;
	reg fault_taken_now;

	wire vs_rise = hdmi_out_vs && !vs_d;
	wire hs_rise = hdmi_out_hs && !hs_d;
	wire de_rise = hdmi_out_de && !de_d;
	wire [15:0] source_base_flags =
		(direct_sync ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS_DIRECT_MUX : 16'd0) |
		(csync_sync ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS_CSYNC_MUX : 16'd0);
	wire [15:0] source_flags = source_base_flags |
		(source_stable ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_SOURCE_FLAGS_MODE_STABLE : 16'd0);
	wire [15:0] live_control_flags =
		(route_flags[0] ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS_MAGIK_OWNERSHIP : 16'd0) |
		(route_flags[2] ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS_ACTIVE_ENABLED : 16'd0) |
		(reset_sync ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS_RESET_REQUEST : 16'd0) |
		(cfg_sync ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS_CONFIGURATION_DONE : 16'd0) |
		(pll_sync ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_CONTROL_FLAGS_PLL_ADJUST_LOCKED : 16'd0);
	wire [15:0] live_frame_flags =
		(saw_de ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_SAW_DE : 16'd0) |
		(saw_nonblack ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_SAW_NONBLACK : 16'd0) |
		(saw_nonwhite ? MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_SAW_NONWHITE : 16'd0);
	// A complete raster with no active-data interval is also a black-output
	// failure: requiring DE would make a lost-DE black screen invisible.
	wire frame_is_black = !saw_de || !saw_nonblack;
	wire frame_is_white = saw_de && !saw_nonwhite;
	wire frame_timing_changed = reference_valid &&
		((frame_period != reference_period) || (line_count != reference_lines) ||
		 (active_pixels != reference_pixels) || (active_lines != reference_active_lines));

	task automatic freeze_fault;
		input [7:0] new_trigger;
		input [15:0] new_flags;
		input [15:0] new_geometry;
		begin
			if(armed && !frozen && !fault_taken_now) begin
				fault_taken_now = 1'b1;
				frozen <= 1'b1;
				fault_trigger <= new_trigger;
				fault_flags <= live_frame_flags | new_flags;
				geometry_faults <= new_geometry;
				snapshot_source_flags <= source_flags;
				snapshot_control_flags <= live_control_flags;
				snapshot_heartbeat <= heartbeat_toggle;
				fault_notify_pending <= 1'b1;
			end
		end
	endtask

	wire [15:0] snapshot_state_word =
		(frozen ? (MAGIK_VIDEO_DIAGNOSTICS_STATE_FROZEN |
			MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_MONITOR_ARMED |
			MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_SNAPSHOT_FROZEN |
			(mailbox_overrun ? MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_MAILBOX_OVERRUN : 16'd0)) :
		 (armed ? (MAGIK_VIDEO_DIAGNOSTICS_STATE_ARMED |
			MAGIK_VIDEO_DIAGNOSTICS_STATE_FLAGS_MONITOR_ARMED) :
			MAGIK_VIDEO_DIAGNOSTICS_STATE_IDLE));
	assign snapshot_payload = {
		16'd0, 16'd0, control_changes, {15'd0, snapshot_heartbeat}, frame_count[31:16],
		frame_count[15:0], geometry_faults, {14'd0, consecutive_white},
		{14'd0, consecutive_black}, fault_flags, fault_active_lines,
		fault_pixels[31:16], fault_pixels[15:0], fault_lines, fault_period[31:16],
		fault_period[15:0], reference_flags, reference_active_lines,
		reference_pixels[31:16], reference_pixels[15:0], reference_lines,
		reference_period[31:16], reference_period[15:0], snapshot_control_flags,
		snapshot_source_flags, active_sequence, route_epoch, snapshot_generation,
		{8'd0, fault_trigger}, snapshot_state_word, MAGIK_VIDEO_DIAGNOSTICS_SCHEMA};

	always @(posedge hdmi_tx_clk) begin
		fault_taken_now = 1'b0;
		armed_meta <= monitor_armed_async;
		armed <= armed_meta;
		armed_d <= armed;
		request_meta <= snapshot_request_toggle_async;
		request_sync <= request_meta;
		route_meta <= route_context_toggle_async;
		route_sync <= route_meta;
		direct_meta <= mux_direct_async;
		direct_sync <= direct_meta;
		csync_meta <= mux_csync_async;
		csync_sync <= csync_meta;
		reset_meta <= reset_req_async;
		reset_sync <= reset_meta;
		cfg_meta <= cfg_done_async;
		cfg_sync <= cfg_meta;
		pll_meta <= pll_adjust_locked_async;
		pll_sync <= pll_meta;
		vs_d <= hdmi_out_vs;
		hs_d <= hdmi_out_hs;
		de_d <= hdmi_out_de;
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
		    ({route_epoch, active_sequence, route_flags} !=
		     {expected_route_epoch_async, expected_active_seq_async,
		      expected_route_flags_async})))) begin
			if(route_capture_pending) mailbox_overrun <= 1'b1;
			route_seen <= route_sync;
			route_epoch <= expected_route_epoch_async;
			active_sequence <= expected_active_seq_async;
			route_flags <= expected_route_flags_async;
			route_capture_pending <= 1'b1;
		end
		else if(!frozen && route_capture_pending) begin
			if({route_epoch, active_sequence, route_flags} ==
			   {expected_route_epoch_async, expected_active_seq_async,
				expected_route_flags_async})
				route_capture_pending <= 1'b0;
			else begin
				route_epoch <= expected_route_epoch_async;
				active_sequence <= expected_active_seq_async;
				route_flags <= expected_route_flags_async;
			end
		end

		if(armed && !armed_d) begin
			have_frame <= 1'b0;
			reference_valid <= 1'b0;
			consecutive_black <= 2'd0;
			consecutive_white <= 2'd0;
			consecutive_timing <= 2'd0;
			source_stable <= 1'b0;
			last_frame_source <= source_base_flags;
		end

		if(!frozen) begin
			if(frame_period != 32'hffffffff) frame_period <= frame_period + 1'd1;
			else fault_flags <= fault_flags |
				MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_COUNTER_OVERFLOW;
			if(hs_rise && line_count != 16'hffff) line_count <= line_count + 1'd1;
			if(de_rise && active_lines != 16'hffff) active_lines <= active_lines + 1'd1;
			if(hdmi_out_de) begin
				saw_de <= 1'b1;
				if(|hdmi_out_d) saw_nonblack <= 1'b1;
				if(!(&hdmi_out_d)) saw_nonwhite <= 1'b1;
				if(active_pixels != 32'hffffffff) active_pixels <= active_pixels + 1'd1;
			end
		end

		if(vs_rise) begin
			heartbeat_toggle <= ~heartbeat_toggle;
			if(!frozen) begin
				if(frame_count != 32'hffffffff) frame_count <= frame_count + 1'd1;
				if(source_base_flags == last_frame_source) source_stable <= 1'b1;
				else begin
					source_stable <= 1'b0;
					last_frame_source <= source_base_flags;
				end
			end
			if(armed && have_frame && !frozen) begin
				fault_period <= frame_period;
				fault_lines <= line_count;
				fault_pixels <= active_pixels;
				fault_active_lines <= active_lines;
				if(!reference_valid && source_stable && saw_de && saw_nonblack && saw_nonwhite) begin
					reference_valid <= 1'b1;
					reference_period <= frame_period;
					reference_lines <= line_count;
					reference_pixels <= active_pixels;
					reference_active_lines <= active_lines;
					reference_flags <= live_frame_flags;
					reference_source_flags <= source_flags;
				end
				if(frame_is_black) begin
					if(consecutive_black < 3) consecutive_black <= consecutive_black + 1'd1;
					if(consecutive_black == 1)
						freeze_fault(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK,
							MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_ALL_BLACK, 16'd0);
				end
				else consecutive_black <= 2'd0;
				if(frame_is_white) begin
					if(consecutive_white < 3) consecutive_white <= consecutive_white + 1'd1;
					if(consecutive_white == 1)
						freeze_fault(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_WHITE,
							MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_ALL_WHITE, 16'd0);
				end
				else consecutive_white <= 2'd0;
				if(frame_timing_changed) begin
					if(consecutive_timing < 3) consecutive_timing <= consecutive_timing + 1'd1;
					if(consecutive_timing == 1)
						freeze_fault(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING,
							MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_TIMING_CHANGED,
							{12'd0, active_lines != reference_active_lines,
							 active_pixels != reference_pixels, line_count != reference_lines,
							 frame_period != reference_period});
				end
				else consecutive_timing <= 2'd0;
				if(reference_valid && (source_flags != reference_source_flags)) begin
					control_changes <= control_changes + 1'd1;
					freeze_fault(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING,
						MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_MUX_CHANGED, 16'd0);
				end
			end
			if(!frozen) begin
				have_frame <= 1'b1;
				frame_period <= 32'd0;
				line_count <= 16'd0;
				active_lines <= 16'd0;
				active_pixels <= 32'd0;
				saw_de <= 1'b0;
				saw_nonblack <= 1'b0;
				saw_nonwhite <= 1'b0;
			end
		end

		if(request_sync != request_seen) begin
			request_seen <= request_sync;
			snapshot_generation <= diagnostic_generation_async;
			request_capture_pending <= 1'b1;
		end
		else if(request_capture_pending) begin
			if(snapshot_generation == diagnostic_generation_async) begin
				request_capture_pending <= 1'b0;
				if(!frozen) begin
					frozen <= 1'b1;
					snapshot_source_flags <= source_flags;
					snapshot_control_flags <= live_control_flags;
					snapshot_heartbeat <= heartbeat_toggle;
				end
				request_ack_pending <= 1'b1;
			end
			else snapshot_generation <= diagnostic_generation_async;
		end
	end

endmodule

`default_nettype wire
