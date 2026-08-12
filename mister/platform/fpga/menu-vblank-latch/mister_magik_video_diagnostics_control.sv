// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Passive, configuration-lifetime evidence for the physical HDMI FPLL lock.
// Its only output is the dedicated read-only UIO response pair.
module mister_magik_hdmi_lock_evidence (
	input  wire        clk_sys,
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	input  wire        hdmi_pll_locked,
	output wire        response_valid,
	output reg  [15:0] response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	reg       has_command = 1'b0;
	reg       command_selected = 1'b0;
	reg [2:0] word_count = 3'd0;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start = io_din[7:0] == MAGIK_UIO_GET_HDMI_EVIDENCE;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command && (word_count < MAGIK_HDMI_EVIDENCE_WORDS));

	// Preserve these exact named stages. The raw status path is excluded only
	// into the first stage; the first-to-second settling path remains timed.
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg control_pll_lock_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg control_pll_lock_sys = 1'b0;

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
		case(word_count)
			MAGIK_HDMI_EVIDENCE_SCHEMA_WORD:
				response_word = MAGIK_HDMI_EVIDENCE_SCHEMA;
			MAGIK_HDMI_EVIDENCE_FLAGS_WORD:
				response_word = {11'd0, snapshot_flags};
			MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD:
				response_word = snapshot_lock_loss_count;
			default: response_word = tx_crc;
		endcase

		response_data = 16'd0;
		if(command_start && selected_start)
			response_data = MAGIK_HDMI_EVIDENCE_MAGIC;
		else if(command_data && selected_command &&
			(word_count < MAGIK_HDMI_EVIDENCE_WORDS))
			response_data = response_word;
	end

	always @(posedge clk_sys) begin
		control_pll_lock_meta <= hdmi_pll_locked;
		control_pll_lock_sys <= control_pll_lock_meta;
		lock_previous <= control_pll_lock_sys;
		lock_seen_high <= lock_seen_high_next;
		lock_armed <= lock_armed_next;
		lock_ever_lost <= lock_ever_lost_next;
		lock_loss_count <= lock_loss_count_next;
		lock_loss_count_overflow <= lock_loss_count_overflow_next;

		if(command_start) begin
			has_command <= 1'b1;
			command_selected <= selected_start;
			word_count <= 3'd0;
			if(selected_start) begin
				snapshot_flags <= evidence_flags_next[4:0];
				snapshot_lock_loss_count <= lock_loss_count_next;
				tx_crc <= MAGIK_HDMI_EVIDENCE_HEADER_CRC;
			end
		end
		else if(command_data && selected_command &&
			(word_count < MAGIK_HDMI_EVIDENCE_WORDS)) begin
			word_count <= word_count + 1'd1;
			if(word_count < MAGIK_HDMI_EVIDENCE_CRC_WORD)
				tx_crc <= crc_update_word(tx_crc, response_word);
		end

		if(!io_uio && has_command) begin
			has_command <= 1'b0;
			command_selected <= 1'b0;
			word_count <= 3'd0;
		end
	end
endmodule

`default_nettype wire
