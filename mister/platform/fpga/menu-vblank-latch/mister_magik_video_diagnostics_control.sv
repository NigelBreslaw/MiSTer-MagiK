// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Passive, configuration-lifetime evidence for the physical HDMI FPLL lock
// and completed activity at the final registered HDMI output. Its only output
// is the dedicated read-only UIO response pair.
module mister_magik_hdmi_lock_evidence (
	input  wire        clk_sys,
	input  wire        hdmi_tx_clk,
	input  wire        clk_hdmi,
	input  wire        clk_100m,
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	input  wire        hdmi_pll_locked,
	input  wire        hdmi_out_vs,
	input  wire        hdmi_out_de,
	input  wire [23:0] hdmi_out_d,
	input  wire        hdmi_out_direct,
	input  wire        scaler_raw_vs,
	input  wire        scaler_raw_de,
	input  wire [23:0] scaler_raw_d,
	input  wire        post_osd_vs,
	input  wire        post_osd_de,
	input  wire [23:0] post_osd_d,
	input  wire        vbuf_read,
	input  wire        vbuf_waitrequest,
	input  wire        vbuf_readdatavalid,
	output wire        response_valid,
	output reg  [15:0] response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	reg       has_command = 1'b0;
	reg [2:0] command_kind = 3'd0;
	reg [2:0] word_count = 3'd0;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire lock_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_EVIDENCE;
	wire activity_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY;
	wire final_path_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY;
	wire scaler_raw_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY;
	wire post_osd_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY;
	wire avalon_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY;
	wire selected_start = lock_start || activity_start || final_path_start ||
		scaler_raw_start || post_osd_start || avalon_start;
	wire selected_command = command_kind != 3'd0;
	wire [2:0] selected_words =
		(command_kind == 3'd1) ? MAGIK_HDMI_EVIDENCE_WORDS :
		(command_kind == 3'd2) ? MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS :
		(command_kind == 3'd3) ? MAGIK_HDMI_FINAL_PATH_ACTIVITY_WORDS :
		(command_kind == 3'd4) ? MAGIK_HDMI_SCALER_RAW_ACTIVITY_WORDS :
		(command_kind == 3'd5) ? MAGIK_HDMI_POST_OSD_ACTIVITY_WORDS :
		(command_kind == 3'd6) ? MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_WORDS : 3'd0;
	wire [2:0] selected_crc_word =
		(command_kind == 3'd1) ? {1'b0, MAGIK_HDMI_EVIDENCE_CRC_WORD} :
		(command_kind == 3'd2) ? MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD :
		(command_kind == 3'd3) ? MAGIK_HDMI_FINAL_PATH_ACTIVITY_CRC_WORD :
		(command_kind == 3'd4) ? MAGIK_HDMI_SCALER_RAW_ACTIVITY_CRC_WORD :
		(command_kind == 3'd5) ? MAGIK_HDMI_POST_OSD_ACTIVITY_CRC_WORD :
		(command_kind == 3'd6) ? MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD : 3'd0;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command && (word_count < selected_words));

	// Classify only the final registered values that directly drive the HDMI
	// transmitter pins. The first rising VS arms and discards the partial
	// configuration-start interval. Every later rising VS toggles exactly one
	// completed-frame class.
	reg output_vs_previous = 1'b0;
	reg output_frame_armed = 1'b0;
	reg output_frame_saw_de = 1'b0;
	reg output_frame_saw_nonzero = 1'b0;
	reg output_frame_saw_direct = 1'b0;
	reg output_frame_saw_scaled = 1'b0;
	reg output_no_de_toggle = 1'b0;
	reg output_de_has_nonzero_toggle = 1'b0;
	reg output_black_direct_toggle = 1'b0;
	reg output_black_scaled_toggle = 1'b0;
	reg output_black_mixed_toggle = 1'b0;
	wire output_vs_rise = hdmi_out_vs && !output_vs_previous;
	wire output_sample_nonzero = hdmi_out_de && (|hdmi_out_d);
	wire output_frame_saw_de_now = output_frame_saw_de || hdmi_out_de;
	wire output_frame_saw_nonzero_now =
		output_frame_saw_nonzero || output_sample_nonzero;
	wire output_frame_saw_direct_now =
		output_frame_saw_direct || (hdmi_out_de && hdmi_out_direct);
	wire output_frame_saw_scaled_now =
		output_frame_saw_scaled || (hdmi_out_de && !hdmi_out_direct);

	always @(posedge hdmi_tx_clk) begin
		output_vs_previous <= hdmi_out_vs;
		if(output_vs_rise) begin
			if(output_frame_armed) begin
				if(!output_frame_saw_de_now)
					output_no_de_toggle <= !output_no_de_toggle;
				else if(output_frame_saw_nonzero_now)
					output_de_has_nonzero_toggle <= !output_de_has_nonzero_toggle;
				else if(output_frame_saw_direct_now && output_frame_saw_scaled_now)
					output_black_mixed_toggle <= !output_black_mixed_toggle;
				else if(output_frame_saw_direct_now)
					output_black_direct_toggle <= !output_black_direct_toggle;
				else
					output_black_scaled_toggle <= !output_black_scaled_toggle;
			end
			output_frame_armed <= 1'b1;
			output_frame_saw_de <= 1'b0;
			output_frame_saw_nonzero <= 1'b0;
			output_frame_saw_direct <= 1'b0;
			output_frame_saw_scaled <= 1'b0;
		end
		else begin
			output_frame_saw_de <= output_frame_saw_de_now;
			output_frame_saw_nonzero <= output_frame_saw_nonzero_now;
			output_frame_saw_direct <= output_frame_saw_direct_now;
			output_frame_saw_scaled <= output_frame_saw_scaled_now;
		end
	end

	// Independently classify the raw scaler output and the post-OSD scaler
	// branch in their native clk_hdmi domain. These classifiers never exchange
	// payloads with clk_sys; only their mutually exclusive event toggles cross.
	reg raw_vs_previous = 1'b0;
	reg raw_frame_armed = 1'b0;
	reg raw_frame_saw_de = 1'b0;
	reg raw_frame_saw_nonzero = 1'b0;
	reg raw_no_de_toggle = 1'b0;
	reg raw_all_zero_toggle = 1'b0;
	reg raw_nonzero_toggle = 1'b0;
	wire raw_vs_rise = scaler_raw_vs && !raw_vs_previous;
	wire raw_sample_nonzero = scaler_raw_de && (|scaler_raw_d);
	wire raw_frame_saw_de_now = raw_frame_saw_de || scaler_raw_de;
	wire raw_frame_saw_nonzero_now = raw_frame_saw_nonzero || raw_sample_nonzero;

	always @(posedge clk_hdmi) begin
		raw_vs_previous <= scaler_raw_vs;
		if(raw_vs_rise) begin
			if(raw_frame_armed) begin
				if(!raw_frame_saw_de_now)
					raw_no_de_toggle <= !raw_no_de_toggle;
				else if(raw_frame_saw_nonzero_now)
					raw_nonzero_toggle <= !raw_nonzero_toggle;
				else
					raw_all_zero_toggle <= !raw_all_zero_toggle;
			end
			raw_frame_armed <= 1'b1;
			raw_frame_saw_de <= 1'b0;
			raw_frame_saw_nonzero <= 1'b0;
		end
		else begin
			raw_frame_saw_de <= raw_frame_saw_de_now;
			raw_frame_saw_nonzero <= raw_frame_saw_nonzero_now;
		end
	end

	reg post_vs_previous = 1'b0;
	reg post_frame_armed = 1'b0;
	reg post_frame_saw_de = 1'b0;
	reg post_frame_saw_nonzero = 1'b0;
	reg post_no_de_toggle = 1'b0;
	reg post_all_zero_toggle = 1'b0;
	reg post_nonzero_toggle = 1'b0;
	wire post_vs_rise = post_osd_vs && !post_vs_previous;
	wire post_sample_nonzero = post_osd_de && (|post_osd_d);
	wire post_frame_saw_de_now = post_frame_saw_de || post_osd_de;
	wire post_frame_saw_nonzero_now = post_frame_saw_nonzero || post_sample_nonzero;

	always @(posedge clk_hdmi) begin
		post_vs_previous <= post_osd_vs;
		if(post_vs_rise) begin
			if(post_frame_armed) begin
				if(!post_frame_saw_de_now)
					post_no_de_toggle <= !post_no_de_toggle;
				else if(post_frame_saw_nonzero_now)
					post_nonzero_toggle <= !post_nonzero_toggle;
				else
					post_all_zero_toggle <= !post_all_zero_toggle;
			end
			post_frame_armed <= 1'b1;
			post_frame_saw_de <= 1'b0;
			post_frame_saw_nonzero <= 1'b0;
		end
		else begin
			post_frame_saw_de <= post_frame_saw_de_now;
			post_frame_saw_nonzero <= post_frame_saw_nonzero_now;
		end
	end

	// Aggregate only transport liveness in fixed 2^19-cycle clk_100m buckets.
	// No address or read data crosses domains. A separate bucket heartbeat makes
	// a valid all-zero bucket distinguishable from a stopped source clock.
	reg [18:0] avalon_bucket_count = 19'd0;
	reg avalon_bucket_saw_request = 1'b0;
	reg avalon_bucket_saw_accepted = 1'b0;
	reg avalon_bucket_saw_returned = 1'b0;
	reg avalon_bucket_toggle = 1'b0;
	reg avalon_request_toggle = 1'b0;
	reg avalon_accepted_toggle = 1'b0;
	reg avalon_returned_toggle = 1'b0;
	wire avalon_request_now = avalon_bucket_saw_request || vbuf_read;
	wire avalon_accepted_now = avalon_bucket_saw_accepted ||
		(vbuf_read && !vbuf_waitrequest);
	wire avalon_returned_now = avalon_bucket_saw_returned || vbuf_readdatavalid;

	always @(posedge clk_100m) begin
		if(&avalon_bucket_count) begin
			avalon_bucket_count <= 19'd0;
			avalon_bucket_toggle <= !avalon_bucket_toggle;
			if(avalon_request_now)
				avalon_request_toggle <= !avalon_request_toggle;
			if(avalon_accepted_now)
				avalon_accepted_toggle <= !avalon_accepted_toggle;
			if(avalon_returned_now)
				avalon_returned_toggle <= !avalon_returned_toggle;
			avalon_bucket_saw_request <= 1'b0;
			avalon_bucket_saw_accepted <= 1'b0;
			avalon_bucket_saw_returned <= 1'b0;
		end
		else begin
			avalon_bucket_count <= avalon_bucket_count + 1'd1;
			avalon_bucket_saw_request <= avalon_request_now;
			avalon_bucket_saw_accepted <= avalon_accepted_now;
			avalon_bucket_saw_returned <= avalon_returned_now;
		end
	end

	// Preserve these exact named stages. The raw status path is excluded only
	// into the first stage; the first-to-second settling path remains timed.
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg control_pll_lock_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg control_pll_lock_sys = 1'b0;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg output_no_de_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_no_de_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg output_black_direct_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_black_direct_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg output_black_scaled_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_black_scaled_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg output_black_mixed_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_black_mixed_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg output_de_has_nonzero_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_de_has_nonzero_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg raw_no_de_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg raw_no_de_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg raw_all_zero_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg raw_all_zero_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg raw_nonzero_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg raw_nonzero_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg post_no_de_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg post_no_de_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg post_all_zero_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg post_all_zero_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg post_nonzero_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg post_nonzero_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg avalon_bucket_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg avalon_bucket_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg avalon_request_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg avalon_request_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg avalon_accepted_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg avalon_accepted_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg avalon_returned_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg avalon_returned_sys = 1'b0;

	reg [3:0] output_no_de_count = 4'd0;
	reg [3:0] output_de_has_nonzero_count = 4'd0;
	reg [3:0] output_black_direct_count = 4'd0;
	reg [3:0] output_black_scaled_count = 4'd0;
	reg [3:0] output_black_mixed_count = 4'd0;
	reg output_frame_valid = 1'b0;
	reg output_counter_collision = 1'b0;
	// Each accepted event flips both the source toggle and the counter LSB, so
	// the counter itself is also the last-seen token. This avoids three separate
	// destination history registers.
	wire output_no_de_event = output_no_de_sys != output_no_de_count[0];
	wire output_de_has_nonzero_event =
		output_de_has_nonzero_sys != output_de_has_nonzero_count[0];
	wire output_black_direct_event =
		output_black_direct_sys != output_black_direct_count[0];
	wire output_black_scaled_event =
		output_black_scaled_sys != output_black_scaled_count[0];
	wire output_black_mixed_event =
		output_black_mixed_sys != output_black_mixed_count[0];
	wire output_any_event = output_no_de_event || output_de_has_nonzero_event ||
		output_black_direct_event ||
		output_black_scaled_event || output_black_mixed_event;
	wire output_event_collision =
		(output_no_de_event && output_de_has_nonzero_event) ||
		(output_no_de_event && output_black_direct_event) ||
		(output_no_de_event && output_black_scaled_event) ||
		(output_no_de_event && output_black_mixed_event) ||
		(output_de_has_nonzero_event && output_black_direct_event) ||
		(output_de_has_nonzero_event && output_black_scaled_event) ||
		(output_de_has_nonzero_event && output_black_mixed_event) ||
		(output_black_direct_event && output_black_scaled_event) ||
		(output_black_direct_event && output_black_mixed_event) ||
		(output_black_scaled_event && output_black_mixed_event);
	wire [3:0] output_no_de_count_next =
		output_no_de_count + (output_no_de_event ? 1'd1 : 1'd0);
	wire [3:0] output_de_has_nonzero_count_next =
		output_de_has_nonzero_count + (output_de_has_nonzero_event ? 1'd1 : 1'd0);
	wire [3:0] output_black_direct_count_next =
		output_black_direct_count + (output_black_direct_event ? 1'd1 : 1'd0);
	wire [3:0] output_black_scaled_count_next =
		output_black_scaled_count + (output_black_scaled_event ? 1'd1 : 1'd0);
	wire [3:0] output_black_mixed_count_next =
		output_black_mixed_count + (output_black_mixed_event ? 1'd1 : 1'd0);
	wire [3:0] output_de_all_zero_count_next =
		output_black_direct_count_next + output_black_scaled_count_next +
		output_black_mixed_count_next;
	wire output_frame_valid_next = output_frame_valid || output_any_event;
	wire output_counter_collision_next =
		output_counter_collision || output_event_collision;

	reg [3:0] raw_no_de_count = 4'd0;
	reg [3:0] raw_all_zero_count = 4'd0;
	reg [3:0] raw_nonzero_count = 4'd0;
	reg raw_frame_valid = 1'b0;
	reg raw_counter_collision = 1'b0;
	wire raw_no_de_event = raw_no_de_sys != raw_no_de_count[0];
	wire raw_all_zero_event = raw_all_zero_sys != raw_all_zero_count[0];
	wire raw_nonzero_event = raw_nonzero_sys != raw_nonzero_count[0];
	wire raw_any_event = raw_no_de_event || raw_all_zero_event || raw_nonzero_event;
	wire raw_event_collision =
		(raw_no_de_event && raw_all_zero_event) ||
		(raw_no_de_event && raw_nonzero_event) ||
		(raw_all_zero_event && raw_nonzero_event);
	wire [3:0] raw_no_de_count_next =
		raw_no_de_count + (raw_no_de_event ? 1'd1 : 1'd0);
	wire [3:0] raw_all_zero_count_next =
		raw_all_zero_count + (raw_all_zero_event ? 1'd1 : 1'd0);
	wire [3:0] raw_nonzero_count_next =
		raw_nonzero_count + (raw_nonzero_event ? 1'd1 : 1'd0);
	wire raw_frame_valid_next = raw_frame_valid || raw_any_event;
	wire raw_counter_collision_next = raw_counter_collision || raw_event_collision;

	reg [3:0] post_no_de_count = 4'd0;
	reg [3:0] post_all_zero_count = 4'd0;
	reg [3:0] post_nonzero_count = 4'd0;
	reg post_frame_valid = 1'b0;
	reg post_counter_collision = 1'b0;
	wire post_no_de_event = post_no_de_sys != post_no_de_count[0];
	wire post_all_zero_event = post_all_zero_sys != post_all_zero_count[0];
	wire post_nonzero_event = post_nonzero_sys != post_nonzero_count[0];
	wire post_any_event =
		post_no_de_event || post_all_zero_event || post_nonzero_event;
	wire post_event_collision =
		(post_no_de_event && post_all_zero_event) ||
		(post_no_de_event && post_nonzero_event) ||
		(post_all_zero_event && post_nonzero_event);
	wire [3:0] post_no_de_count_next =
		post_no_de_count + (post_no_de_event ? 1'd1 : 1'd0);
	wire [3:0] post_all_zero_count_next =
		post_all_zero_count + (post_all_zero_event ? 1'd1 : 1'd0);
	wire [3:0] post_nonzero_count_next =
		post_nonzero_count + (post_nonzero_event ? 1'd1 : 1'd0);
	wire post_frame_valid_next = post_frame_valid || post_any_event;
	wire post_counter_collision_next =
		post_counter_collision || post_event_collision;

	reg [3:0] avalon_bucket_epoch = 4'd0;
	reg avalon_bucket_valid = 1'b0;
	reg [3:0] avalon_request_count = 4'd0;
	reg [3:0] avalon_accepted_count = 4'd0;
	reg [3:0] avalon_returned_count = 4'd0;
	wire avalon_bucket_event = avalon_bucket_sys != avalon_bucket_epoch[0];
	wire avalon_request_event = avalon_request_sys != avalon_request_count[0];
	wire avalon_accepted_event = avalon_accepted_sys != avalon_accepted_count[0];
	wire avalon_returned_event = avalon_returned_sys != avalon_returned_count[0];
	wire avalon_bucket_valid_next = avalon_bucket_valid || avalon_bucket_event;
	wire [3:0] avalon_bucket_epoch_next =
		avalon_bucket_epoch + (avalon_bucket_event ? 1'd1 : 1'd0);
	wire [3:0] avalon_request_count_next =
		avalon_request_count + (avalon_request_event ? 1'd1 : 1'd0);
	wire [3:0] avalon_accepted_count_next =
		avalon_accepted_count + (avalon_accepted_event ? 1'd1 : 1'd0);
	wire [3:0] avalon_returned_count_next =
		avalon_returned_count + (avalon_returned_event ? 1'd1 : 1'd0);

	reg lock_previous = 1'b0;
	reg lock_seen_high = 1'b0;
	reg lock_armed = 1'b0;
	reg lock_ever_lost = 1'b0;
	reg lock_loss_count_overflow = 1'b0;
	reg [15:0] lock_loss_count = 16'd0;

	wire lock_loss_event =
		lock_armed && lock_previous && !control_pll_lock_sys;
	wire lock_seen_high_next = lock_seen_high || control_pll_lock_sys;
	// Require two consecutive synchronized high samples before treating a
	// later low sample as a lock loss. A one-sample assertion is evidence that
	// lock was seen, but it does not arm the loss recorder.
	wire lock_armed_next =
		lock_armed || (lock_previous && control_pll_lock_sys);
	wire lock_ever_lost_next = lock_ever_lost || lock_loss_event;
	wire [15:0] lock_loss_count_next =
		!lock_loss_event ? lock_loss_count :
		(lock_loss_count == 16'hffff) ? 16'hffff : lock_loss_count + 1'd1;
	wire lock_loss_count_overflow_next =
		lock_loss_count_overflow ||
		(lock_loss_event && (lock_loss_count == 16'hffff));

	wire [15:0] evidence_flags_next =
		(lock_seen_high_next ? MAGIK_HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH : 16'd0) |
		(lock_armed_next ? MAGIK_HDMI_EVIDENCE_FLAG_LOCK_ARMED : 16'd0) |
		(control_pll_lock_sys ? MAGIK_HDMI_EVIDENCE_FLAG_LOCK_CURRENT : 16'd0) |
		(lock_ever_lost_next ? MAGIK_HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST : 16'd0) |
		(lock_loss_count_overflow_next ?
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_LOSS_COUNT_OVERFLOW : 16'd0);

	reg [4:0] snapshot_flags = 5'd0;
	reg [15:0] snapshot_lock_loss_count = 16'd0;
	reg [15:0] snapshot_path_extra = 16'd0;
	reg [15:0] tx_crc = MAGIK_HDMI_EVIDENCE_HEADER_CRC;
	reg [15:0] response_word;

	function automatic [15:0] crc_update_byte;
		input [15:0] crc_in;
		input [7:0] byte_in;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {byte_in, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : (value << 1);
			crc_update_byte = value;
		end
	endfunction

	function automatic [15:0] crc_update_word;
		input [15:0] crc_in;
		input [15:0] word_in;
		begin
			crc_update_word =
				crc_update_byte(crc_update_byte(crc_in, word_in[15:8]), word_in[7:0]);
		end
	endfunction

	always @(*) begin
		response_word = tx_crc;
		if(command_kind == 3'd1) begin
			case(word_count)
				MAGIK_HDMI_EVIDENCE_SCHEMA_WORD:
					response_word = MAGIK_HDMI_EVIDENCE_SCHEMA;
				MAGIK_HDMI_EVIDENCE_FLAGS_WORD:
					response_word = {11'd0, snapshot_flags};
				MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD:
					response_word = snapshot_lock_loss_count;
				default: response_word = tx_crc;
			endcase
		end
		else if(command_kind == 3'd2) begin
			case(word_count)
				MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA_WORD:
					response_word = MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA;
				MAGIK_HDMI_OUTPUT_ACTIVITY_FLAGS_WORD:
					response_word = {14'd0, snapshot_lock_loss_count[13:12]};
				MAGIK_HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD:
					response_word = {12'd0, snapshot_lock_loss_count[3:0]};
				MAGIK_HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD:
					response_word = {12'd0, snapshot_lock_loss_count[7:4]};
				MAGIK_HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD:
					response_word = {12'd0, snapshot_lock_loss_count[11:8]};
				default: response_word = tx_crc;
			endcase
		end
		else if(command_kind == 3'd3) begin
			case(word_count)
				MAGIK_HDMI_FINAL_PATH_ACTIVITY_SCHEMA_WORD:
					response_word = MAGIK_HDMI_FINAL_PATH_ACTIVITY_SCHEMA;
				MAGIK_HDMI_FINAL_PATH_ACTIVITY_FLAGS_WORD:
					response_word = {11'd0, snapshot_flags};
				MAGIK_HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD:
					response_word = snapshot_lock_loss_count;
				MAGIK_HDMI_FINAL_PATH_ACTIVITY_ACTIVITY_COUNTS_WORD:
					response_word = snapshot_path_extra;
				default: response_word = tx_crc;
			endcase
		end
		else if(command_kind == 3'd4) begin
			case(word_count)
				MAGIK_HDMI_SCALER_RAW_ACTIVITY_SCHEMA_WORD:
					response_word = MAGIK_HDMI_SCALER_RAW_ACTIVITY_SCHEMA;
				MAGIK_HDMI_SCALER_RAW_ACTIVITY_FLAGS_WORD:
					response_word = {11'd0, snapshot_flags};
				MAGIK_HDMI_SCALER_RAW_ACTIVITY_COUNTS_WORD:
					response_word = snapshot_lock_loss_count;
				default: response_word = tx_crc;
			endcase
		end
		else if(command_kind == 3'd5) begin
			case(word_count)
				MAGIK_HDMI_POST_OSD_ACTIVITY_SCHEMA_WORD:
					response_word = MAGIK_HDMI_POST_OSD_ACTIVITY_SCHEMA;
				MAGIK_HDMI_POST_OSD_ACTIVITY_FLAGS_WORD:
					response_word = {11'd0, snapshot_flags};
				MAGIK_HDMI_POST_OSD_ACTIVITY_COUNTS_WORD:
					response_word = snapshot_lock_loss_count;
				default: response_word = tx_crc;
			endcase
		end
		else if(command_kind == 3'd6) begin
			case(word_count)
				MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA_WORD:
					response_word = MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA;
				MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_WORD:
					response_word = {11'd0, snapshot_flags};
				MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD:
					response_word = snapshot_lock_loss_count;
				default: response_word = tx_crc;
			endcase
		end

		response_data = 16'd0;
		if(command_start && lock_start)
			response_data = MAGIK_HDMI_EVIDENCE_MAGIC;
		else if(command_start && activity_start)
			response_data = MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC;
		else if(command_start && final_path_start)
			response_data = MAGIK_HDMI_FINAL_PATH_ACTIVITY_MAGIC;
		else if(command_start && scaler_raw_start)
			response_data = MAGIK_HDMI_SCALER_RAW_ACTIVITY_MAGIC;
		else if(command_start && post_osd_start)
			response_data = MAGIK_HDMI_POST_OSD_ACTIVITY_MAGIC;
		else if(command_start && avalon_start)
			response_data = MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_MAGIC;
		else if(command_data && selected_command &&
			(word_count < selected_words))
			response_data = response_word;
	end

	always @(posedge clk_sys) begin
		control_pll_lock_meta <= hdmi_pll_locked;
		control_pll_lock_sys <= control_pll_lock_meta;
		output_no_de_meta <= output_no_de_toggle;
		output_no_de_sys <= output_no_de_meta;
		output_black_direct_meta <= output_black_direct_toggle;
		output_black_direct_sys <= output_black_direct_meta;
		output_black_scaled_meta <= output_black_scaled_toggle;
		output_black_scaled_sys <= output_black_scaled_meta;
		output_black_mixed_meta <= output_black_mixed_toggle;
		output_black_mixed_sys <= output_black_mixed_meta;
		output_de_has_nonzero_meta <= output_de_has_nonzero_toggle;
		output_de_has_nonzero_sys <= output_de_has_nonzero_meta;
		raw_no_de_meta <= raw_no_de_toggle;
		raw_no_de_sys <= raw_no_de_meta;
		raw_all_zero_meta <= raw_all_zero_toggle;
		raw_all_zero_sys <= raw_all_zero_meta;
		raw_nonzero_meta <= raw_nonzero_toggle;
		raw_nonzero_sys <= raw_nonzero_meta;
		post_no_de_meta <= post_no_de_toggle;
		post_no_de_sys <= post_no_de_meta;
		post_all_zero_meta <= post_all_zero_toggle;
		post_all_zero_sys <= post_all_zero_meta;
		post_nonzero_meta <= post_nonzero_toggle;
		post_nonzero_sys <= post_nonzero_meta;
		avalon_bucket_meta <= avalon_bucket_toggle;
		avalon_bucket_sys <= avalon_bucket_meta;
		avalon_request_meta <= avalon_request_toggle;
		avalon_request_sys <= avalon_request_meta;
		avalon_accepted_meta <= avalon_accepted_toggle;
		avalon_accepted_sys <= avalon_accepted_meta;
		avalon_returned_meta <= avalon_returned_toggle;
		avalon_returned_sys <= avalon_returned_meta;
		output_no_de_count <= output_no_de_count_next;
		output_de_has_nonzero_count <= output_de_has_nonzero_count_next;
		output_black_direct_count <= output_black_direct_count_next;
		output_black_scaled_count <= output_black_scaled_count_next;
		output_black_mixed_count <= output_black_mixed_count_next;
		output_frame_valid <= output_frame_valid_next;
		output_counter_collision <= output_counter_collision_next;
		raw_no_de_count <= raw_no_de_count_next;
		raw_all_zero_count <= raw_all_zero_count_next;
		raw_nonzero_count <= raw_nonzero_count_next;
		raw_frame_valid <= raw_frame_valid_next;
		raw_counter_collision <= raw_counter_collision_next;
		post_no_de_count <= post_no_de_count_next;
		post_all_zero_count <= post_all_zero_count_next;
		post_nonzero_count <= post_nonzero_count_next;
		post_frame_valid <= post_frame_valid_next;
		post_counter_collision <= post_counter_collision_next;
		avalon_bucket_epoch <= avalon_bucket_epoch_next;
		avalon_bucket_valid <= avalon_bucket_valid_next;
		avalon_request_count <= avalon_request_count_next;
		avalon_accepted_count <= avalon_accepted_count_next;
		avalon_returned_count <= avalon_returned_count_next;
		lock_previous <= control_pll_lock_sys;
		lock_seen_high <= lock_seen_high_next;
		lock_armed <= lock_armed_next;
		lock_ever_lost <= lock_ever_lost_next;
		lock_loss_count <= lock_loss_count_next;
		lock_loss_count_overflow <= lock_loss_count_overflow_next;

		if(command_start) begin
			has_command <= 1'b1;
			command_kind <= lock_start ? 3'd1 : activity_start ? 3'd2 :
				final_path_start ? 3'd3 : scaler_raw_start ? 3'd4 :
				post_osd_start ? 3'd5 : avalon_start ? 3'd6 : 3'd0;
			word_count <= 3'd0;
			if(lock_start) begin
				snapshot_flags <= evidence_flags_next[4:0];
				snapshot_lock_loss_count <= lock_loss_count_next;
				tx_crc <= MAGIK_HDMI_EVIDENCE_HEADER_CRC;
			end
			else if(activity_start) begin
				snapshot_lock_loss_count <= {
					2'd0,
					output_counter_collision_next,
					output_frame_valid_next,
					output_de_has_nonzero_count_next,
					output_de_all_zero_count_next,
					output_no_de_count_next
				};
				tx_crc <= MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;
			end
			else if(final_path_start) begin
				snapshot_flags <= {
					3'd0,
					output_counter_collision_next,
					output_frame_valid_next
				};
				snapshot_lock_loss_count <= {
					output_de_has_nonzero_count_next,
					output_black_mixed_count_next,
					output_black_scaled_count_next,
					output_black_direct_count_next
				};
				snapshot_path_extra <= {12'd0, output_no_de_count_next};
				tx_crc <= MAGIK_HDMI_FINAL_PATH_ACTIVITY_HEADER_CRC;
			end
			else if(scaler_raw_start) begin
				snapshot_flags <= {
					3'd0,
					raw_counter_collision_next,
					raw_frame_valid_next
				};
				snapshot_lock_loss_count <= {
					4'd0,
					raw_nonzero_count_next,
					raw_all_zero_count_next,
					raw_no_de_count_next
				};
				tx_crc <= MAGIK_HDMI_SCALER_RAW_ACTIVITY_HEADER_CRC;
			end
			else if(post_osd_start) begin
				snapshot_flags <= {
					3'd0,
					post_counter_collision_next,
					post_frame_valid_next
				};
				snapshot_lock_loss_count <= {
					4'd0,
					post_nonzero_count_next,
					post_all_zero_count_next,
					post_no_de_count_next
				};
				tx_crc <= MAGIK_HDMI_POST_OSD_ACTIVITY_HEADER_CRC;
			end
			else if(avalon_start) begin
				snapshot_flags <= {4'd0, avalon_bucket_valid_next};
				snapshot_lock_loss_count <= {
					avalon_bucket_epoch_next,
					avalon_returned_count_next,
					avalon_accepted_count_next,
					avalon_request_count_next
				};
				tx_crc <= MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_HEADER_CRC;
			end
		end
		else if(command_data && selected_command &&
			(word_count < selected_words)) begin
			word_count <= word_count + 1'd1;
			if(word_count < selected_crc_word)
				tx_crc <= crc_update_word(tx_crc, response_word);
		end

		if(!io_uio && has_command) begin
			has_command <= 1'b0;
			command_kind <= 3'd0;
			word_count <= 3'd0;
		end
	end
endmodule

`default_nettype wire
