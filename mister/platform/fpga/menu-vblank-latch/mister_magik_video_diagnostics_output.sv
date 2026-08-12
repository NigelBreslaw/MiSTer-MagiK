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
	input  wire         hdmi_pll_locked_async,
	input  wire [23:0]  hdmi_out_d,
	input  wire         hdmi_out_de,
	input  wire         hdmi_out_hs,
	input  wire         hdmi_out_vs,
	output reg          heartbeat_toggle = 1'b0,
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
	reg direct_meta = 1'b0, direct_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg csync_meta = 1'b0, csync_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg reset_meta = 1'b0, reset_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg cfg_meta = 1'b0, cfg_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg pll_meta = 1'b0, pll_sync = 1'b0;

	reg armed_d = 1'b0, request_seen = 1'b0, route_seen = 1'b0;
	reg request_capture_pending = 1'b0, request_ack_pending = 1'b0;
	reg route_capture_pending = 1'b0, fault_notify_pending = 1'b0;
	reg [15:0] snapshot_generation = 16'd0;
	reg vs_d = 1'b0, hs_d = 1'b0, de_d = 1'b0;
	reg [15:0] route_epoch = 16'd0, active_sequence = 16'd0;
	reg [4:0] route_flags = 5'd0;
	reg [23:0] frame_period = 24'd0;
	reg [11:0] line_count = 12'd0, active_lines = 12'd0;
	reg saw_de = 1'b0, saw_nonblack = 1'b0, saw_nonwhite = 1'b0;
	reg have_frame = 1'b0, reference_valid = 1'b0;
	reg [23:0] reference_period = 24'd0;
	reg [11:0] reference_lines = 12'd0, reference_active_lines = 12'd0;
	reg [2:0] reference_source_flags = 3'd0;
	reg [1:0] last_frame_source = 2'd0;
	reg source_stable = 1'b0;
	reg [23:0] fault_period = 24'd0;
	reg [7:0] fault_flags = 8'd0;
	reg [2:0] geometry_faults = 3'd0;
	reg [1:0] consecutive_black = 2'd0, consecutive_white = 2'd0;
	reg [1:0] consecutive_timing = 2'd0;
	reg [2:0] snapshot_source_flags = 3'd0;
	reg [4:0] snapshot_control_flags = 5'd0;
	reg frozen = 1'b0, mailbox_overrun = 1'b0;
	reg native_fault_pending = 1'b0;
	reg [7:0] native_fault_trigger = 8'd0;
	reg [7:0] native_fault_flags = 8'd0;
	reg [2:0] native_fault_geometry = 3'd0;
	reg freeze_request_now;
	reg [7:0] freeze_request_trigger;
	reg [7:0] freeze_request_flags;
	reg [2:0] freeze_request_geometry;
	reg [7:0] observed_flags_now;

	wire vs_rise = hdmi_out_vs && !vs_d;
	wire hs_rise = hdmi_out_hs && !hs_d;
	wire de_rise = hdmi_out_de && !de_d;
	wire arm_start = armed && !armed_d;
	wire capture_stopped = frozen || native_fault_pending;
	wire manual_capture_ready = request_capture_pending &&
		(request_sync == request_seen);
	wire [1:0] source_base_flags = {csync_sync, direct_sync};
	wire [2:0] source_flags = {source_stable, csync_sync, direct_sync};
	wire [4:0] live_control_flags =
		{pll_sync, cfg_sync, reset_sync, route_flags[2], route_flags[0]};
	wire [2:0] live_frame_flags = {saw_nonwhite, saw_nonblack, saw_de};
	// A complete raster with no active-data interval is also a black-output
	// failure: requiring DE would make a lost-DE black screen invisible.
	wire frame_is_black = !saw_de || !saw_nonblack;
	wire frame_is_white = saw_de && !saw_nonwhite;
	wire frame_timing_changed = reference_valid &&
		((frame_period != reference_period) ||
		 (line_count != reference_lines) ||
		 (active_lines != reference_active_lines));
	wire [2:0] frame_geometry = {active_lines != reference_active_lines,
		line_count != reference_lines, frame_period != reference_period};
	wire frame_source_changed = reference_valid &&
		(source_flags != reference_source_flags);
	wire [7:0] completed_frame_flags = {{5{1'b0}}, live_frame_flags} | fault_flags;
	task automatic request_freeze;
		input [7:0] new_trigger;
		input [7:0] new_flags;
		input [2:0] new_geometry;
		begin
			if(armed && !capture_stopped && !freeze_request_now) begin
				freeze_request_now = 1'b1;
				freeze_request_trigger = new_trigger;
				freeze_request_flags = new_flags;
				freeze_request_geometry = new_geometry;
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
		{5'd0, geometry_faults, fault_flags},
		{8'd0, fault_period[23:16]}, fault_period[15:0],
		{4'd0, reference_active_lines}, {4'd0, reference_lines},
		{8'd0, reference_period[23:16]}, reference_period[15:0],
		{11'd0, snapshot_control_flags}, {13'd0, snapshot_source_flags},
		active_sequence, route_epoch, snapshot_generation,
		{8'd0, fault_trigger}, snapshot_state_word, MAGIK_VIDEO_DIAGNOSTICS_SCHEMA};

	always @(posedge hdmi_tx_clk) begin
		freeze_request_now = 1'b0;
		freeze_request_trigger = MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_NONE;
		freeze_request_flags = 8'd0;
		freeze_request_geometry = 3'd0;
		observed_flags_now = 8'd0;
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
		pll_meta <= hdmi_pll_locked_async;
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

		if(!capture_stopped && ((route_sync != route_seen) ||
		   (!route_capture_pending &&
		    ({route_epoch, active_sequence, route_flags} !=
		     {expected_route_epoch_async, expected_active_seq_async,
		      expected_route_flags_async[4:0]})))) begin
			if(route_capture_pending) mailbox_overrun <= 1'b1;
			route_seen <= route_sync;
			route_epoch <= expected_route_epoch_async;
			active_sequence <= expected_active_seq_async;
			route_flags <= expected_route_flags_async[4:0];
			route_capture_pending <= 1'b1;
		end
		else if(!capture_stopped && route_capture_pending) begin
			if({route_epoch, active_sequence, route_flags} ==
			   {expected_route_epoch_async, expected_active_seq_async,
				expected_route_flags_async[4:0]})
				route_capture_pending <= 1'b0;
			else begin
				route_epoch <= expected_route_epoch_async;
				active_sequence <= expected_active_seq_async;
				route_flags <= expected_route_flags_async[4:0];
			end
		end

		if(arm_start) begin
			have_frame <= 1'b0;
			reference_valid <= 1'b0;
			consecutive_black <= 2'd0;
			consecutive_white <= 2'd0;
			consecutive_timing <= 2'd0;
			source_stable <= 1'b0;
			last_frame_source <= source_base_flags;
		end

		if(!capture_stopped) begin
			if(frame_period != 24'hffffff) frame_period <= frame_period + 1'd1;
			else observed_flags_now = observed_flags_now |
				MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_COUNTER_OVERFLOW[7:0];
			if(hs_rise && line_count != 12'hfff) line_count <= line_count + 1'd1;
			if(de_rise && active_lines != 12'hfff) active_lines <= active_lines + 1'd1;
			if(hdmi_out_de) begin
				saw_de <= 1'b1;
				if(|hdmi_out_d) saw_nonblack <= 1'b1;
				if(!(&hdmi_out_d)) saw_nonwhite <= 1'b1;
			end
		end

		if(vs_rise) begin
			heartbeat_toggle <= ~heartbeat_toggle;
			if(!capture_stopped && !arm_start) begin
				if(source_base_flags == last_frame_source) source_stable <= 1'b1;
				else begin
					source_stable <= 1'b0;
					last_frame_source <= source_base_flags;
				end
			end
			if(armed && !arm_start && have_frame && !capture_stopped) begin
				fault_period <= frame_period;
				snapshot_source_flags <= source_flags;
				snapshot_control_flags <= live_control_flags;
				if(frame_is_black) begin
					if(consecutive_black < 3)
						consecutive_black <= consecutive_black + 1'd1;
					if(consecutive_black == 1)
						request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_BLACK,
							completed_frame_flags |
							MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_ALL_BLACK,
							3'd0);
				end
				else consecutive_black <= 2'd0;
				if(frame_is_white) begin
					if(consecutive_white < 3)
						consecutive_white <= consecutive_white + 1'd1;
					if(consecutive_white == 1)
						request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_WHITE,
							completed_frame_flags |
							MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_ALL_WHITE,
							3'd0);
				end
				else consecutive_white <= 2'd0;
				if(frame_timing_changed) begin
					if(consecutive_timing < 3)
						consecutive_timing <= consecutive_timing + 1'd1;
					if(consecutive_timing == 1)
						request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING,
							completed_frame_flags |
							MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_TIMING_CHANGED,
							frame_geometry);
				end
				else consecutive_timing <= 2'd0;
				if(frame_source_changed)
					request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_FINAL_TIMING,
						completed_frame_flags |
						MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_FAULT_FLAGS_MUX_CHANGED,
						3'd0);
				if(!reference_valid && source_stable && saw_de && saw_nonblack && saw_nonwhite) begin
					reference_valid <= 1'b1;
					reference_period <= frame_period;
					reference_lines <= line_count;
					reference_active_lines <= active_lines;
					reference_source_flags <= source_flags;
				end
			end
			if(!capture_stopped && !arm_start) begin
				have_frame <= 1'b1;
				frame_period <= 24'd0;
				line_count <= 12'd0;
				active_lines <= 12'd0;
				saw_de <= 1'b0;
				saw_nonblack <= 1'b0;
				saw_nonwhite <= 1'b0;
			end
		end

		if(request_sync != request_seen) begin
			request_seen <= request_sync;
			request_capture_pending <= 1'b1;
		end

		// Serialize native and manual freezes. A selected native first fault owns
		// the record even when a manual request is already waiting; that request
		// attaches its generation and acknowledgement to the committed native
		// evidence without recapturing later source or control state.
		if(native_fault_pending) begin
			native_fault_pending <= 1'b0;
			frozen <= 1'b1;
			fault_trigger <= native_fault_trigger;
			fault_flags <= native_fault_flags;
			geometry_faults <= native_fault_geometry;
			fault_notify_pending <= 1'b1;
			if(manual_capture_ready) begin
				snapshot_generation <= diagnostic_generation_async;
				request_capture_pending <= 1'b0;
				request_ack_pending <= 1'b1;
			end
		end
		else if(freeze_request_now) begin
			native_fault_pending <= 1'b1;
			native_fault_trigger <= freeze_request_trigger;
			native_fault_flags <= freeze_request_flags | observed_flags_now;
			native_fault_geometry <= freeze_request_geometry;
		end
		else if(manual_capture_ready) begin
			snapshot_generation <= diagnostic_generation_async;
			request_capture_pending <= 1'b0;
			if(!frozen) begin
				frozen <= 1'b1;
				snapshot_source_flags <= source_flags;
				snapshot_control_flags <= live_control_flags;
			end
			request_ack_pending <= 1'b1;
			if(observed_flags_now != 0)
				fault_flags <= fault_flags | observed_flags_now;
		end
		else if(observed_flags_now != 0)
			fault_flags <= fault_flags | observed_flags_now;
	end

endmodule

`default_nettype wire
