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
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	input  wire        hdmi_pll_locked,
	input  wire        hdmi_out_vs,
	input  wire        hdmi_out_de,
	input  wire [23:0] hdmi_out_d,
	output wire        response_valid,
	output reg  [15:0] response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	reg       has_command = 1'b0;
	reg [1:0] command_kind = 2'd0;
	reg [2:0] word_count = 3'd0;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire lock_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_EVIDENCE;
	wire activity_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY;
	wire selected_start = lock_start || activity_start;
	wire selected_command = command_kind != 2'd0;
	wire [2:0] selected_words =
		(command_kind == 2'd1) ? MAGIK_HDMI_EVIDENCE_WORDS :
		(command_kind == 2'd2) ? MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS : 3'd0;
	wire [2:0] selected_crc_word =
		(command_kind == 2'd1) ? {1'b0, MAGIK_HDMI_EVIDENCE_CRC_WORD} :
		(command_kind == 2'd2) ? MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD : 3'd0;

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
	reg output_no_de_toggle = 1'b0;
	reg output_de_all_zero_toggle = 1'b0;
	reg output_de_has_nonzero_toggle = 1'b0;
	wire output_vs_rise = hdmi_out_vs && !output_vs_previous;
	wire output_sample_nonzero = hdmi_out_de && (|hdmi_out_d);
	wire output_frame_saw_de_now = output_frame_saw_de || hdmi_out_de;
	wire output_frame_saw_nonzero_now =
		output_frame_saw_nonzero || output_sample_nonzero;

	always @(posedge hdmi_tx_clk) begin
		output_vs_previous <= hdmi_out_vs;
		if(output_vs_rise) begin
			if(output_frame_armed) begin
				if(!output_frame_saw_de_now)
					output_no_de_toggle <= !output_no_de_toggle;
				else if(!output_frame_saw_nonzero_now)
					output_de_all_zero_toggle <= !output_de_all_zero_toggle;
				else
					output_de_has_nonzero_toggle <= !output_de_has_nonzero_toggle;
			end
			output_frame_armed <= 1'b1;
			output_frame_saw_de <= 1'b0;
			output_frame_saw_nonzero <= 1'b0;
		end
		else begin
			output_frame_saw_de <= output_frame_saw_de_now;
			output_frame_saw_nonzero <= output_frame_saw_nonzero_now;
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
	reg output_de_all_zero_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_de_all_zero_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg output_de_has_nonzero_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_de_has_nonzero_sys = 1'b0;

	reg [3:0] output_no_de_count = 4'd0;
	reg [3:0] output_de_all_zero_count = 4'd0;
	reg [3:0] output_de_has_nonzero_count = 4'd0;
	reg output_frame_valid = 1'b0;
	reg output_counter_collision = 1'b0;
	// Each accepted event flips both the source toggle and the counter LSB, so
	// the counter itself is also the last-seen token. This avoids three separate
	// destination history registers.
	wire output_no_de_event = output_no_de_sys != output_no_de_count[0];
	wire output_de_all_zero_event =
		output_de_all_zero_sys != output_de_all_zero_count[0];
	wire output_de_has_nonzero_event =
		output_de_has_nonzero_sys != output_de_has_nonzero_count[0];
	wire output_any_event = output_no_de_event || output_de_all_zero_event ||
		output_de_has_nonzero_event;
	wire output_event_collision =
		(output_no_de_event && output_de_all_zero_event) ||
		(output_no_de_event && output_de_has_nonzero_event) ||
		(output_de_all_zero_event && output_de_has_nonzero_event);
	wire [3:0] output_no_de_count_next =
		output_no_de_count + (output_no_de_event ? 1'd1 : 1'd0);
	wire [3:0] output_de_all_zero_count_next =
		output_de_all_zero_count + (output_de_all_zero_event ? 1'd1 : 1'd0);
	wire [3:0] output_de_has_nonzero_count_next =
		output_de_has_nonzero_count + (output_de_has_nonzero_event ? 1'd1 : 1'd0);
	wire output_frame_valid_next = output_frame_valid || output_any_event;
	wire output_counter_collision_next =
		output_counter_collision || output_event_collision;

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
		if(command_kind == 2'd1) begin
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
		else if(command_kind == 2'd2) begin
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

		response_data = 16'd0;
		if(command_start && lock_start)
			response_data = MAGIK_HDMI_EVIDENCE_MAGIC;
		else if(command_start && activity_start)
			response_data = MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC;
		else if(command_data && selected_command &&
			(word_count < selected_words))
			response_data = response_word;
	end

	always @(posedge clk_sys) begin
		control_pll_lock_meta <= hdmi_pll_locked;
		control_pll_lock_sys <= control_pll_lock_meta;
		output_no_de_meta <= output_no_de_toggle;
		output_no_de_sys <= output_no_de_meta;
		output_de_all_zero_meta <= output_de_all_zero_toggle;
		output_de_all_zero_sys <= output_de_all_zero_meta;
		output_de_has_nonzero_meta <= output_de_has_nonzero_toggle;
		output_de_has_nonzero_sys <= output_de_has_nonzero_meta;
		output_no_de_count <= output_no_de_count_next;
		output_de_all_zero_count <= output_de_all_zero_count_next;
		output_de_has_nonzero_count <= output_de_has_nonzero_count_next;
		output_frame_valid <= output_frame_valid_next;
		output_counter_collision <= output_counter_collision_next;
		lock_previous <= control_pll_lock_sys;
		lock_seen_high <= lock_seen_high_next;
		lock_armed <= lock_armed_next;
		lock_ever_lost <= lock_ever_lost_next;
		lock_loss_count <= lock_loss_count_next;
		lock_loss_count_overflow <= lock_loss_count_overflow_next;

		if(command_start) begin
			has_command <= 1'b1;
			command_kind <= lock_start ? 2'd1 : activity_start ? 2'd2 : 2'd0;
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
		end
		else if(command_data && selected_command &&
			(word_count < selected_words)) begin
			word_count <= word_count + 1'd1;
			if(word_count < selected_crc_word)
				tx_crc <= crc_update_word(tx_crc, response_word);
		end

		if(!io_uio && has_command) begin
			has_command <= 1'b0;
			command_kind <= 2'd0;
			word_count <= 3'd0;
		end
	end
endmodule

`default_nettype wire
