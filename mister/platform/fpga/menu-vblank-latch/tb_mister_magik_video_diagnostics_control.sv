// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Test-only serializer for focused recorder tests. Production uses the shared
// latch parser/CRC bank exercised by tb_mister_magik_sys_top_integration.
module mister_magik_hdmi_evidence_test_wrapper (
	input wire clk_sys, input wire hdmi_tx_clk, input wire clk_hdmi,
	input wire clk_100m, input wire io_uio, input wire io_strobe,
	input wire [15:0] io_din, input wire hdmi_pll_locked,
	input wire hdmi_out_vs, input wire hdmi_out_de,
	input wire [23:0] hdmi_out_d, input wire hdmi_out_direct,
	input wire scaler_raw_vs, input wire scaler_raw_de,
	input wire [23:0] scaler_raw_d, input wire post_osd_vs,
	input wire post_osd_de, input wire [23:0] post_osd_d,
	input wire vbuf_read, input wire vbuf_waitrequest,
	input wire vbuf_readdatavalid,
	input wire scaler_fetch_batch_two_toggle,
	input wire scaler_fetch_starved_frame_toggle,
	input wire scaler_fetch_snapshot_valid,
	input wire scaler_fetch_delta_invalid,
	input wire scaler_fetch_level_invalid,
	output wire response_valid, output reg [15:0] response_data
);
	`include "mister_magik_video_diagnostics_protocol.svh"

	wire [15:0] evidence_word0;
	wire [15:0] evidence_word1;
	wire [15:0] evidence_word2;
	wire [15:0] evidence_word3;
	wire [15:0] evidence_word4;
	reg has_command = 1'b0;
	reg [7:0] command = 8'd0;
	reg [2:0] word_count = 3'd0;
	reg [15:0] snapshot [0:4];
	reg [15:0] tx_crc = 16'd0;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start = (io_din[7:0] >= MAGIK_UIO_GET_HDMI_EVIDENCE) &&
		(io_din[7:0] <= MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY);
	wire selected_command = (command >= MAGIK_UIO_GET_HDMI_EVIDENCE) &&
		(command <= MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY);
	wire [2:0] selected_words =
		(command == MAGIK_UIO_GET_HDMI_EVIDENCE) ? MAGIK_HDMI_EVIDENCE_WORDS :
		(command == MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY) ?
			MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS :
		(command == MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY) ?
			MAGIK_HDMI_FINAL_PATH_ACTIVITY_WORDS :
		(command == MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY) ?
			MAGIK_HDMI_SCALER_RAW_ACTIVITY_WORDS :
		(command == MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY) ?
			MAGIK_HDMI_POST_OSD_ACTIVITY_WORDS :
		(command == MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY) ?
			MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_WORDS :
		(command == MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY) ?
			MAGIK_HDMI_SCALER_FETCH_ACTIVITY_WORDS : 3'd0;
	wire [2:0] crc_word_index = selected_words - 1'd1;
	assign response_valid = (command_start && selected_start) ||
		(command_data && selected_command && (word_count < selected_words));

	function automatic [15:0] crc_byte;
		input [15:0] crc_in; input [7:0] data;
		integer bit_index; reg [15:0] value;
		begin
			value = crc_in ^ {data, 8'd0};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : value << 1;
			crc_byte = value;
		end
	endfunction
	function automatic [15:0] crc_word;
		input [15:0] crc_in; input [15:0] data;
		begin crc_word = crc_byte(crc_byte(crc_in, data[15:8]), data[7:0]); end
	endfunction

	mister_magik_hdmi_lock_evidence recorder (
		.clk_sys(clk_sys), .hdmi_tx_clk(hdmi_tx_clk), .clk_hdmi(clk_hdmi),
		.clk_100m(clk_100m), .evidence_command(io_din[7:0]),
		.hdmi_pll_locked(hdmi_pll_locked), .hdmi_out_vs(hdmi_out_vs),
		.hdmi_out_de(hdmi_out_de), .hdmi_out_d(hdmi_out_d),
		.hdmi_out_direct(hdmi_out_direct), .scaler_raw_vs(scaler_raw_vs),
		.scaler_raw_de(scaler_raw_de), .scaler_raw_d(scaler_raw_d),
		.post_osd_vs(post_osd_vs), .post_osd_de(post_osd_de),
		.post_osd_d(post_osd_d), .vbuf_read(vbuf_read),
		.vbuf_waitrequest(vbuf_waitrequest),
		.vbuf_readdatavalid(vbuf_readdatavalid),
		.scaler_fetch_batch_two_toggle(scaler_fetch_batch_two_toggle),
		.scaler_fetch_starved_frame_toggle(scaler_fetch_starved_frame_toggle),
		.scaler_fetch_snapshot_valid(scaler_fetch_snapshot_valid),
		.scaler_fetch_delta_invalid(scaler_fetch_delta_invalid),
		.scaler_fetch_level_invalid(scaler_fetch_level_invalid),
		.evidence_word0(evidence_word0), .evidence_word1(evidence_word1),
		.evidence_word2(evidence_word2), .evidence_word3(evidence_word3),
		.evidence_word4(evidence_word4)
	);

	always @(*) begin
		response_data = 16'd0;
		if(command_start) begin
			case(io_din[7:0])
				MAGIK_UIO_GET_HDMI_EVIDENCE: response_data = MAGIK_HDMI_EVIDENCE_MAGIC;
				MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY:
					response_data = MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC;
				MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY:
					response_data = MAGIK_HDMI_FINAL_PATH_ACTIVITY_MAGIC;
				MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY:
					response_data = MAGIK_HDMI_SCALER_RAW_ACTIVITY_MAGIC;
				MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY:
					response_data = MAGIK_HDMI_POST_OSD_ACTIVITY_MAGIC;
				MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY:
					response_data = MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_MAGIC;
				MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY:
					response_data = MAGIK_HDMI_SCALER_FETCH_ACTIVITY_MAGIC;
				default: response_data = 16'd0;
			endcase
		end
		else if(command_data && selected_command) begin
			if(word_count < crc_word_index) response_data = snapshot[word_count];
			else if(word_count == crc_word_index) response_data = tx_crc;
		end
	end

	always @(posedge clk_sys) begin
		if(!io_uio) begin
			has_command <= 1'b0; command <= 8'd0; word_count <= 3'd0;
		end
		else if(command_start) begin
			has_command <= 1'b1; command <= io_din[7:0]; word_count <= 3'd0;
			snapshot[0] <= evidence_word0; snapshot[1] <= evidence_word1;
			snapshot[2] <= evidence_word2; snapshot[3] <= evidence_word3;
			snapshot[4] <= evidence_word4;
			case(io_din[7:0])
				MAGIK_UIO_GET_HDMI_EVIDENCE: tx_crc <= MAGIK_HDMI_EVIDENCE_HEADER_CRC;
				MAGIK_UIO_GET_HDMI_OUTPUT_ACTIVITY:
					tx_crc <= MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;
				MAGIK_UIO_GET_HDMI_FINAL_PATH_ACTIVITY:
					tx_crc <= MAGIK_HDMI_FINAL_PATH_ACTIVITY_HEADER_CRC;
				MAGIK_UIO_GET_HDMI_SCALER_RAW_ACTIVITY:
					tx_crc <= MAGIK_HDMI_SCALER_RAW_ACTIVITY_HEADER_CRC;
				MAGIK_UIO_GET_HDMI_POST_OSD_ACTIVITY:
					tx_crc <= MAGIK_HDMI_POST_OSD_ACTIVITY_HEADER_CRC;
				MAGIK_UIO_GET_HDMI_AVALON_LIVENESS_ACTIVITY:
					tx_crc <= MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_HEADER_CRC;
				MAGIK_UIO_GET_HDMI_SCALER_FETCH_ACTIVITY:
					tx_crc <= MAGIK_HDMI_SCALER_FETCH_ACTIVITY_HEADER_CRC;
				default: tx_crc <= 16'd0;
			endcase
		end
		else if(command_data && selected_command && (word_count < selected_words)) begin
			word_count <= word_count + 1'd1;
			if(word_count < crc_word_index) tx_crc <= crc_word(tx_crc, snapshot[word_count]);
		end
	end
endmodule

module tb_mister_magik_video_diagnostics_control;
`include "mister_magik_video_diagnostics_protocol.svh"

	reg clk_sys = 1'b0;
	always #5 clk_sys = ~clk_sys;
	reg hdmi_tx_clk = 1'b0;
	always #7 hdmi_tx_clk = ~hdmi_tx_clk;
	reg clk_hdmi = 1'b0;
	always #11 clk_hdmi = ~clk_hdmi;
	reg clk_100m = 1'b0;
	always #4 clk_100m = ~clk_100m;

	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg [15:0] io_din = 16'd0;
	reg hdmi_pll_locked = 1'b0;
	reg hdmi_out_vs = 1'b0;
	reg hdmi_out_de = 1'b0;
	reg [23:0] hdmi_out_d = 24'd0;
	reg hdmi_out_direct = 1'b0;
	reg scaler_raw_vs = 1'b0;
	reg scaler_raw_de = 1'b0;
	reg [23:0] scaler_raw_d = 24'd0;
	reg post_osd_vs = 1'b0;
	reg post_osd_de = 1'b0;
	reg [23:0] post_osd_d = 24'd0;
	reg vbuf_read = 1'b0;
	reg vbuf_waitrequest = 1'b0;
	reg vbuf_readdatavalid = 1'b0;
	reg scaler_fetch_batch_two_toggle = 1'b0;
	reg scaler_fetch_starved_frame_toggle = 1'b0;
	reg scaler_fetch_snapshot_valid = 1'b0;
	reg scaler_fetch_delta_invalid = 1'b0;
	reg scaler_fetch_level_invalid = 1'b0;
	wire response_valid;
	wire [15:0] response_data;
	integer index;
	reg [15:0] words [0:5];
	reg [15:0] crc;
	reg [15:0] armed_flags;
	reg [15:0] lost_flags;

	mister_magik_hdmi_evidence_test_wrapper dut (
		.clk_sys(clk_sys),
		.hdmi_tx_clk(hdmi_tx_clk),
		.clk_hdmi(clk_hdmi),
		.clk_100m(clk_100m),
		.io_uio(io_uio),
		.io_strobe(io_strobe),
		.io_din(io_din),
		.hdmi_pll_locked(hdmi_pll_locked),
		.hdmi_out_vs(hdmi_out_vs),
		.hdmi_out_de(hdmi_out_de),
		.hdmi_out_d(hdmi_out_d),
		.hdmi_out_direct(hdmi_out_direct),
		.scaler_raw_vs(scaler_raw_vs),
		.scaler_raw_de(scaler_raw_de),
		.scaler_raw_d(scaler_raw_d),
		.post_osd_vs(post_osd_vs),
		.post_osd_de(post_osd_de),
		.post_osd_d(post_osd_d),
		.vbuf_read(vbuf_read),
		.vbuf_waitrequest(vbuf_waitrequest),
		.vbuf_readdatavalid(vbuf_readdatavalid),
		.scaler_fetch_batch_two_toggle(scaler_fetch_batch_two_toggle),
		.scaler_fetch_starved_frame_toggle(scaler_fetch_starved_frame_toggle),
		.scaler_fetch_snapshot_valid(scaler_fetch_snapshot_valid),
		.scaler_fetch_delta_invalid(scaler_fetch_delta_invalid),
		.scaler_fetch_level_invalid(scaler_fetch_level_invalid),
		.response_valid(response_valid),
		.response_data(response_data)
	);

	function automatic [15:0] crc_byte;
		input [15:0] crc_in;
		input [7:0] data;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {data, 8'd0};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : value << 1;
			crc_byte = value;
		end
	endfunction

	function automatic [15:0] crc_word;
		input [15:0] crc_in;
		input [15:0] data;
		begin
			crc_word = crc_byte(crc_byte(crc_in, data[15:8]), data[7:0]);
		end
	endfunction

	task automatic close_command;
		begin
			@(negedge clk_sys); io_uio = 1'b0; io_strobe = 1'b0;
			@(negedge clk_sys);
		end
	endtask

	task automatic read_activity_snapshot_while_frame_completes;
		input [15:0] expected_flags;
		input [7:0] expected_no_de;
		input [7:0] expected_all_zero;
		input [7:0] expected_nonzero;
		begin
			io_uio = 1'b1;
			io_din = 16'h0061; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC)
				$fatal(1, "missing activity magic before concurrent frame");
			@(negedge clk_sys); io_strobe = 1'b0;
			complete_output_frame(1'b1, 1'b1, 1'b0);
			crc = MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_strobe = 1'b1;
				#1 words[index] = response_data;
				if(index < MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			close_command();
			if(words[MAGIK_HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] != {8'd0, expected_no_de} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD] !=
					{8'd0, expected_all_zero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD] !=
					{8'd0, expected_nonzero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD] != crc)
				$fatal(1, "activity command-start snapshot changed mid-read");
		end
	endtask

	task automatic pulse_scaler_vs;
		input raw_path;
		begin
			@(negedge clk_hdmi);
			if(raw_path) scaler_raw_vs = 1'b1;
			else post_osd_vs = 1'b1;
			@(negedge clk_hdmi);
			if(raw_path) scaler_raw_vs = 1'b0;
			else post_osd_vs = 1'b0;
		end
	endtask

	task automatic complete_scaler_frame;
		input raw_path;
		input saw_de;
		input saw_nonzero;
		begin
			@(negedge clk_hdmi);
			if(raw_path) begin
				scaler_raw_de = saw_de;
				scaler_raw_d = saw_nonzero ? 24'h112233 : 24'd0;
			end
			else begin
				post_osd_de = saw_de;
				post_osd_d = saw_nonzero ? 24'h445566 : 24'd0;
			end
			@(negedge clk_hdmi);
			if(raw_path) begin
				scaler_raw_de = 1'b0;
				scaler_raw_d = 24'd0;
			end
			else begin
				post_osd_de = 1'b0;
				post_osd_d = 24'd0;
			end
			pulse_scaler_vs(raw_path);
			repeat(8) @(negedge clk_sys);
		end
	endtask

	task automatic read_scaler_activity;
		input raw_path;
		input [15:0] expected_flags;
		input [3:0] expected_no_de;
		input [3:0] expected_all_zero;
		input [3:0] expected_nonzero;
		reg [15:0] expected_magic;
		reg [15:0] header_crc;
		begin
			expected_magic = raw_path ? MAGIK_HDMI_SCALER_RAW_ACTIVITY_MAGIC :
				MAGIK_HDMI_POST_OSD_ACTIVITY_MAGIC;
			header_crc = raw_path ? MAGIK_HDMI_SCALER_RAW_ACTIVITY_HEADER_CRC :
				MAGIK_HDMI_POST_OSD_ACTIVITY_HEADER_CRC;
			io_uio = 1'b1;
			io_din = raw_path ? 16'h0063 : 16'h0064;
			io_strobe = 1'b1;
			#1 if(!response_valid || response_data != expected_magic)
				$fatal(1, "missing scaler activity magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			crc = header_crc;
			for(index = 0; index < 4; index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "scaler activity ended early");
				words[index] = response_data;
				if(index < 3) crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			@(negedge clk_sys); io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "scaler activity exceeded word count");
			@(negedge clk_sys); io_strobe = 1'b0;
			close_command();
			if(words[0] != 16'd1 || words[1] != expected_flags ||
			   words[2] != {4'd0, expected_nonzero, expected_all_zero,
					expected_no_de} || words[3] != crc)
				$fatal(1, "scaler activity mismatch raw=%0d", raw_path);
		end
	endtask

	task automatic read_avalon_activity;
		input [15:0] expected_flags;
		input [3:0] expected_bucket;
		input [3:0] expected_request;
		input [3:0] expected_accepted;
		input [3:0] expected_returned;
		begin
			io_uio = 1'b1; io_din = 16'h0065; io_strobe = 1'b1;
			#1 if(!response_valid ||
				response_data != MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_MAGIC)
				$fatal(1, "missing Avalon liveness magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			crc = MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_WORDS;
				index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "Avalon liveness ended early");
				words[index] = response_data;
				if(index < MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			close_command();
			if(words[MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA_WORD] !=
					MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_SCHEMA ||
			   words[MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_WORD] !=
					expected_flags ||
			   words[MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD] !=
					{expected_bucket, expected_returned, expected_accepted,
					 expected_request} ||
			   words[MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_CRC_WORD] != crc)
				$fatal(1, "Avalon liveness mismatch flags=%h counts=%h expected=%h",
					words[MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_FLAGS_WORD],
					words[MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_COUNTS_WORD],
					{expected_bucket, expected_returned, expected_accepted,
					 expected_request});
		end
	endtask

	task automatic read_scaler_fetch_activity;
		input [15:0] expected_state;
		input [15:0] expected_events;
		input [15:0] expected_flags;
		begin
			io_uio = 1'b1; io_din = 16'h0066; io_strobe = 1'b1;
			#1 if(!response_valid ||
				response_data != MAGIK_HDMI_SCALER_FETCH_ACTIVITY_MAGIC)
				$fatal(1, "missing scaler fetch activity magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			crc = MAGIK_HDMI_SCALER_FETCH_ACTIVITY_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_SCALER_FETCH_ACTIVITY_WORDS;
				index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "scaler fetch activity ended early");
				words[index] = response_data;
				if(index < MAGIK_HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			@(negedge clk_sys); io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "scaler fetch activity exceeded word count");
			@(negedge clk_sys); io_strobe = 1'b0;
			close_command();
			if(words[MAGIK_HDMI_SCALER_FETCH_ACTIVITY_SCHEMA_WORD] !=
					MAGIK_HDMI_SCALER_FETCH_ACTIVITY_SCHEMA ||
			   words[MAGIK_HDMI_SCALER_FETCH_ACTIVITY_STATE_WORD] != expected_state ||
			   words[MAGIK_HDMI_SCALER_FETCH_ACTIVITY_EVENTS_WORD] != expected_events ||
			   words[MAGIK_HDMI_SCALER_FETCH_ACTIVITY_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_SCALER_FETCH_ACTIVITY_CRC_WORD] != crc)
				$fatal(1, "scaler fetch activity mismatch");
		end
	endtask

	task automatic drive_synchronized_lock;
		input value;
		begin
			hdmi_pll_locked = value;
			while(dut.recorder.control_pll_lock_sys !== value) @(negedge clk_sys);
			if(value)
				while(dut.recorder.lock_armed !== 1'b1) @(negedge clk_sys);
		end
	endtask

	task automatic pulse_output_vs;
		begin
			@(negedge hdmi_tx_clk); hdmi_out_vs = 1'b1;
			@(negedge hdmi_tx_clk); hdmi_out_vs = 1'b0;
		end
	endtask

	task automatic complete_output_frame;
		input saw_de;
		input saw_nonzero;
		input selected_direct;
		begin
			@(negedge hdmi_tx_clk);
			hdmi_out_vs = 1'b0;
			hdmi_out_de = saw_de;
			hdmi_out_d = saw_nonzero ? 24'h010203 : 24'd0;
			hdmi_out_direct = selected_direct;
			@(negedge hdmi_tx_clk);
			hdmi_out_de = 1'b0;
			hdmi_out_d = 24'd0;
			pulse_output_vs();
			repeat(8) @(negedge clk_sys);
		end
	endtask

	task automatic read_final_path_activity;
		input [15:0] expected_flags;
		input [3:0] expected_direct_black;
		input [3:0] expected_scaled_black;
		input [3:0] expected_mixed_black;
		input [3:0] expected_nonzero;
		input [3:0] expected_no_de;
		begin
			io_uio = 1'b1;
			io_din = 16'h0062; io_strobe = 1'b1;
			#1 if(!response_valid ||
				response_data != MAGIK_HDMI_FINAL_PATH_ACTIVITY_MAGIC)
				$fatal(1, "missing HDMI final-path activity magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			crc = MAGIK_HDMI_FINAL_PATH_ACTIVITY_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_FINAL_PATH_ACTIVITY_WORDS;
				index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid)
					$fatal(1, "final-path activity ended at word %0d", index);
				words[index] = response_data;
				if(index < MAGIK_HDMI_FINAL_PATH_ACTIVITY_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			@(negedge clk_sys); io_strobe = 1'b1;
			#1 if(response_valid)
				$fatal(1, "final-path activity exceeded fixed word count");
			@(negedge clk_sys); io_strobe = 1'b0;
			close_command();
			if(words[MAGIK_HDMI_FINAL_PATH_ACTIVITY_SCHEMA_WORD] !=
					MAGIK_HDMI_FINAL_PATH_ACTIVITY_SCHEMA ||
			   words[MAGIK_HDMI_FINAL_PATH_ACTIVITY_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_FINAL_PATH_ACTIVITY_BLACK_COUNTS_WORD] !=
					{expected_nonzero, expected_mixed_black,
					 expected_scaled_black, expected_direct_black} ||
			   words[MAGIK_HDMI_FINAL_PATH_ACTIVITY_ACTIVITY_COUNTS_WORD] !=
					{12'd0, expected_no_de} ||
			   words[MAGIK_HDMI_FINAL_PATH_ACTIVITY_CRC_WORD] != crc)
				$fatal(1, "HDMI final-path activity mismatch");
		end
	endtask

	task automatic read_evidence;
		input [15:0] expected_flags;
		input [15:0] expected_loss_count;
		begin
			io_uio = 1'b1;
			io_din = 16'h0060; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_EVIDENCE_MAGIC)
				$fatal(1, "missing HDMI lock evidence magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			#1 if(response_valid) $fatal(1, "response remained valid between strobes");
			crc = MAGIK_HDMI_EVIDENCE_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_EVIDENCE_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "HDMI evidence ended at word %0d", index);
				words[index] = response_data;
				if(index < MAGIK_HDMI_EVIDENCE_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
				#1 if(response_valid) $fatal(1, "word response remained valid between strobes");
			end
			@(negedge clk_sys); io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "HDMI evidence exceeded fixed word count");
			@(negedge clk_sys); io_strobe = 1'b0;
			close_command();
			if(words[MAGIK_HDMI_EVIDENCE_SCHEMA_WORD] != MAGIK_HDMI_EVIDENCE_SCHEMA ||
			   words[MAGIK_HDMI_EVIDENCE_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD] != expected_loss_count ||
			   words[MAGIK_HDMI_EVIDENCE_CRC_WORD] != crc)
				$fatal(1, "HDMI evidence mismatch flags=%h count=%h crc=%h/%h",
					words[MAGIK_HDMI_EVIDENCE_FLAGS_WORD],
					words[MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD],
					words[MAGIK_HDMI_EVIDENCE_CRC_WORD], crc);
		end
	endtask

	task automatic read_output_activity;
		input [15:0] expected_flags;
		input [7:0] expected_no_de;
		input [7:0] expected_all_zero;
		input [7:0] expected_nonzero;
		begin
			io_uio = 1'b1;
			io_din = 16'h0061; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_OUTPUT_ACTIVITY_MAGIC)
				$fatal(1, "missing HDMI output activity magic");
			@(negedge clk_sys); io_strobe = 1'b0;
			#1 if(response_valid) $fatal(1, "activity response remained valid between strobes");
			crc = MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_OUTPUT_ACTIVITY_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
				#1;
				if(!response_valid) $fatal(1, "HDMI activity ended at word %0d", index);
				words[index] = response_data;
				if(index < MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
				#1 if(response_valid) $fatal(1, "activity word remained valid between strobes");
			end
			@(negedge clk_sys); io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "HDMI activity exceeded fixed word count");
			@(negedge clk_sys); io_strobe = 1'b0;
			close_command();
			if(words[MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA_WORD] !=
					MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_FLAGS_WORD] != expected_flags ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD] != {8'd0, expected_no_de} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD] !=
					{8'd0, expected_all_zero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD] !=
					{8'd0, expected_nonzero} ||
			   words[MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD] != crc)
				$fatal(1, "HDMI activity mismatch flags=%h counts=%h/%h/%h crc=%h/%h",
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_FLAGS_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_NO_DE_COUNT_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_ALL_ZERO_COUNT_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_DE_HAS_NONZERO_COUNT_WORD],
					words[MAGIK_HDMI_OUTPUT_ACTIVITY_CRC_WORD], crc);
		end
	endtask

	task automatic read_armed_snapshot_while_lock_falls;
		begin
			io_uio = 1'b1; io_din = 16'h0060; io_strobe = 1'b1;
			#1 if(!response_valid || response_data != MAGIK_HDMI_EVIDENCE_MAGIC)
				$fatal(1, "missing magic before concurrent loss");
			@(negedge clk_sys); io_strobe = 1'b0; hdmi_pll_locked = 1'b0;
			crc = MAGIK_HDMI_EVIDENCE_HEADER_CRC;
			for(index = 0; index < MAGIK_HDMI_EVIDENCE_WORDS; index = index + 1) begin
				@(negedge clk_sys); io_strobe = 1'b1;
				#1 words[index] = response_data;
				if(index < MAGIK_HDMI_EVIDENCE_CRC_WORD)
					crc = crc_word(crc, response_data);
				@(negedge clk_sys); io_strobe = 1'b0;
			end
			close_command();
			if(words[MAGIK_HDMI_EVIDENCE_FLAGS_WORD] != armed_flags ||
			   words[MAGIK_HDMI_EVIDENCE_LOCK_LOSS_COUNT_WORD] != 16'd0 ||
			   words[MAGIK_HDMI_EVIDENCE_CRC_WORD] != crc)
				$fatal(1, "command-start snapshot changed during concurrent loss");
		end
	endtask

	integer opcode;
	initial begin
		armed_flags = MAGIK_HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_ARMED |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_CURRENT;
		lost_flags = MAGIK_HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_ARMED |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST;

		repeat(4) @(negedge clk_sys);
		if(MAGIK_HDMI_EVIDENCE_HEADER_CRC != 16'h109f)
			$fatal(1, "unexpected generated HDMI evidence header CRC");
		if(MAGIK_HDMI_OUTPUT_ACTIVITY_HEADER_CRC != 16'hda08)
			$fatal(1, "unexpected generated HDMI output activity header CRC");
		if(MAGIK_HDMI_FINAL_PATH_ACTIVITY_HEADER_CRC != 16'h24fb)
			$fatal(1, "unexpected generated HDMI final-path header CRC");
		if(MAGIK_HDMI_SCALER_RAW_ACTIVITY_HEADER_CRC != 16'hfe4d ||
		   MAGIK_HDMI_POST_OSD_ACTIVITY_HEADER_CRC != 16'h9999)
			$fatal(1, "unexpected generated scaler activity header CRC");
		if(MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_HEADER_CRC != 16'h33c8)
			$fatal(1, "unexpected generated Avalon liveness header CRC");
		if(MAGIK_HDMI_SCALER_FETCH_ACTIVITY_HEADER_CRC != 16'hadfd)
			$fatal(1, "unexpected generated scaler fetch header CRC");

		// The responder must ignore all latch-owned and retired commands.
		for(opcode = 8'h57; opcode <= 8'h5f; opcode = opcode + 1) begin
			io_uio = 1'b1;
			io_din = opcode[15:0]; io_strobe = 1'b1;
			#1 if(response_valid) $fatal(1, "lock evidence responded to opcode %h", opcode);
			close_command();
		end
		io_uio = 1'b0; io_din = 16'h0060; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "lock evidence responded outside UIO");
		@(negedge clk_sys); io_strobe = 1'b0;

		// A different command cannot become selected by changing io_din later.
		io_uio = 1'b1; io_din = 16'h0067; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "unknown command unexpectedly selected");
		@(negedge clk_sys); io_strobe = 1'b0;
		@(negedge clk_sys); io_din = 16'h0060; io_strobe = 1'b1;
		#1 if(response_valid) $fatal(1, "active command morphed into HDMI evidence");
		close_command();

		read_evidence(16'd0, 16'd0);
		read_output_activity(16'd0, 8'd0, 8'd0, 8'd0);

		// The first rising VS only arms and discards the partial interval.
		pulse_output_vs();
		repeat(8) @(negedge clk_sys);
		read_output_activity(16'd0, 8'd0, 8'd0, 8'd0);

		complete_output_frame(1'b0, 1'b0, 1'b0);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd1, 8'd0, 8'd0);
		complete_output_frame(1'b1, 1'b0, 1'b0);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd1, 8'd1, 8'd0);
		complete_output_frame(1'b1, 1'b1, 1'b0);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd1, 8'd1, 8'd1);
		read_final_path_activity(MAGIK_HDMI_FINAL_PATH_ACTIVITY_FLAG_FRAME_VALID,
			4'd0, 4'd1, 4'd0, 4'd1, 4'd1);

		// Nonzero RGB outside DE must not be reported as active nonzero video.
		@(negedge hdmi_tx_clk); hdmi_out_d = 24'hffffff;
		@(negedge hdmi_tx_clk); hdmi_out_d = 24'd0;
		pulse_output_vs();
		repeat(8) @(negedge clk_sys);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd2, 8'd1, 8'd1);

		// A long VS level is one edge, not multiple completed frames.
		@(negedge hdmi_tx_clk); hdmi_out_de = 1'b1; hdmi_out_d = 24'd0;
		@(negedge hdmi_tx_clk); hdmi_out_de = 1'b0;
		hdmi_out_vs = 1'b1;
		repeat(4) @(negedge hdmi_tx_clk);
		hdmi_out_vs = 1'b0;
		repeat(8) @(negedge clk_sys);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd2, 8'd2, 8'd1);

		// A command snapshots atomically even if another completed frame arrives
		// before the payload words are streamed.
		read_activity_snapshot_while_frame_completes(
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID, 8'd2, 8'd2, 8'd1);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd2, 8'd2, 8'd2);

		// Counters are explicitly modulo four-bit epochs; wrapping remains a
		// valid snapshot for host-side modular delta calculation.
		@(negedge clk_sys);
		dut.recorder.output_no_de_toggle = 1'b1;
		dut.recorder.output_no_de_meta = 1'b1;
		dut.recorder.output_no_de_sys = 1'b1;
		dut.recorder.output_no_de_count = 4'hf;
		complete_output_frame(1'b0, 1'b0, 1'b0);
		read_output_activity(MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID,
			8'd0, 8'd2, 8'd2);

		// The source classifier cannot emit two classes for one frame. If CDC
		// capture nevertheless sees simultaneous channels, retain a sticky
		// integrity flag instead of silently presenting coherent evidence.
		@(negedge hdmi_tx_clk);
		dut.recorder.output_black_direct_toggle =
			!dut.recorder.output_black_direct_toggle;
		dut.recorder.output_de_has_nonzero_toggle =
			!dut.recorder.output_de_has_nonzero_toggle;
		repeat(8) @(negedge clk_sys);
		read_output_activity(
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID |
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_COUNTER_COLLISION,
			8'd0, 8'd3, 8'd3);

		// Provenance is sampled only during DE. A fully black frame can be
		// direct-only, scaled-only, or mixed if the functional mux changes
		// during active video; it must never be guessed from a later live bit.
		complete_output_frame(1'b1, 1'b0, 1'b1);
		@(negedge hdmi_tx_clk);
		hdmi_out_vs = 1'b0; hdmi_out_de = 1'b1;
		hdmi_out_d = 24'd0; hdmi_out_direct = 1'b1;
		@(negedge hdmi_tx_clk); hdmi_out_direct = 1'b0;
		@(negedge hdmi_tx_clk); hdmi_out_de = 1'b0;
		pulse_output_vs();
		repeat(8) @(negedge clk_sys);
		read_final_path_activity(
			MAGIK_HDMI_FINAL_PATH_ACTIVITY_FLAG_FRAME_VALID |
			MAGIK_HDMI_FINAL_PATH_ACTIVITY_FLAG_COUNTER_COLLISION,
			4'd2, 4'd2, 4'd1, 4'd3, 4'd0);

		// The raw scaler and post-OSD classifiers are independent even though
		// they share clk_hdmi. Each discards its first partial frame and then
		// reports the same three exhaustive activity classes.
		pulse_scaler_vs(1'b1);
		pulse_scaler_vs(1'b0);
		complete_scaler_frame(1'b1, 1'b0, 1'b0);
		complete_scaler_frame(1'b1, 1'b1, 1'b0);
		complete_scaler_frame(1'b1, 1'b1, 1'b1);
		read_scaler_activity(1'b1,
			MAGIK_HDMI_SCALER_RAW_ACTIVITY_FLAG_FRAME_VALID,
			4'd1, 4'd1, 4'd1);
		complete_scaler_frame(1'b0, 1'b1, 1'b0);
		complete_scaler_frame(1'b0, 1'b1, 1'b1);
		complete_scaler_frame(1'b0, 1'b0, 1'b0);
		read_scaler_activity(1'b0,
			MAGIK_HDMI_POST_OSD_ACTIVITY_FLAG_FRAME_VALID,
			4'd1, 4'd1, 4'd1);

		// Close a bounded Avalon bucket with all three liveness facts, then a
		// second empty bucket. The heartbeat makes the empty interval valid while
		// the category epochs correctly remain unchanged.
		@(negedge clk_100m);
		dut.recorder.avalon_bucket_count = 19'h7fffe;
		vbuf_read = 1'b1;
		vbuf_waitrequest = 1'b0;
		vbuf_readdatavalid = 1'b1;
		repeat(2) @(posedge clk_100m);
		@(negedge clk_100m);
		vbuf_read = 1'b0;
		vbuf_readdatavalid = 1'b0;
		repeat(8) @(negedge clk_sys);
		read_avalon_activity(
			MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID,
			4'd1, 4'd1, 4'd1, 4'd1);
		@(negedge clk_100m); dut.recorder.avalon_bucket_count = 19'h7ffff;
		@(posedge clk_100m);
		repeat(8) @(negedge clk_sys);
		read_avalon_activity(
			MAGIK_HDMI_AVALON_LIVENESS_ACTIVITY_FLAG_BUCKET_VALID,
			4'd2, 4'd1, 4'd1, 4'd1);

		// The scaler-fetch record preserves only independently synchronized event
		// epochs. Its retired live-state word remains strict zero.
		scaler_fetch_batch_two_toggle = !scaler_fetch_batch_two_toggle;
		scaler_fetch_starved_frame_toggle = !scaler_fetch_starved_frame_toggle;
		repeat(8) @(negedge clk_sys);
		read_scaler_fetch_activity(16'd0, 16'd0, 16'd0);
		scaler_fetch_snapshot_valid = 1'b1;
		repeat(8) @(negedge clk_sys);
		read_scaler_fetch_activity(
			16'd0, 16'h0011,
			MAGIK_HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID);
		scaler_fetch_delta_invalid = 1'b1;
		scaler_fetch_level_invalid = 1'b1;
		repeat(8) @(negedge clk_sys);
		read_scaler_fetch_activity(
			16'd0, 16'h0011,
			MAGIK_HDMI_SCALER_FETCH_ACTIVITY_FLAG_SNAPSHOT_VALID |
			MAGIK_HDMI_SCALER_FETCH_ACTIVITY_FLAG_COMPLETION_DELTA_INVALID |
			MAGIK_HDMI_SCALER_FETCH_ACTIVITY_FLAG_COMPLETION_LEVEL_INVALID);

		// An aborted activity transaction restarts from its own schema and CRC
		// seed without perturbing the permanent 0x60 lock record.
		io_uio = 1'b1;
		@(negedge clk_sys); io_din = 16'h0061; io_strobe = 1'b1;
		@(negedge clk_sys); io_strobe = 1'b0;
		@(negedge clk_sys); io_strobe = 1'b1;
		#1 if(response_data != MAGIK_HDMI_OUTPUT_ACTIVITY_SCHEMA)
			$fatal(1, "activity transaction did not start at schema");
		close_command();
		read_output_activity(
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_FRAME_VALID |
			MAGIK_HDMI_OUTPUT_ACTIVITY_FLAG_COUNTER_COLLISION,
			8'd0, 8'd5, 8'd3);

		// A raw pulse sampled by the first stage exactly once produces one high
		// sample at the synchronized stage. It records that lock was seen but
		// must not arm the loss counter.
		@(negedge clk_sys); hdmi_pll_locked = 1'b1;
		@(negedge clk_sys); hdmi_pll_locked = 1'b0;
		while(dut.recorder.control_pll_lock_sys !== 1'b1) @(negedge clk_sys);
		while(dut.recorder.control_pll_lock_sys !== 1'b0) @(negedge clk_sys);
		read_evidence(MAGIK_HDMI_EVIDENCE_FLAG_LOCK_SEEN_HIGH, 16'd0);

		drive_synchronized_lock(1'b1);
		read_evidence(armed_flags, 16'd0);
		read_armed_snapshot_while_lock_falls();
		read_evidence(lost_flags, 16'd1);

		// Recover, then start on the first destination half-cycle after another
		// synchronized fall,
		// before sticky state receives its next edge. The command snapshot must
		// include the same pending loss transition through *_next.
		drive_synchronized_lock(1'b1);
		drive_synchronized_lock(1'b0);
		if(dut.recorder.lock_loss_event !== 1'b1)
			$fatal(1, "test missed the uncommitted synchronized loss edge");
		read_evidence(lost_flags, 16'd2);

		// Recovery changes only the live-current flag. A second loss increments
		// the sticky counter and retains the first-fault evidence.
		drive_synchronized_lock(1'b1);
		read_evidence(armed_flags | MAGIK_HDMI_EVIDENCE_FLAG_LOCK_EVER_LOST, 16'd2);
		drive_synchronized_lock(1'b0);
		read_evidence(lost_flags, 16'd3);

		// An aborted transaction must restart from schema word zero.
		io_uio = 1'b1;
		@(negedge clk_sys); io_din = 16'h0060; io_strobe = 1'b1;
		@(negedge clk_sys); io_strobe = 1'b0;
		@(negedge clk_sys); io_strobe = 1'b1;
		#1 if(response_data != MAGIK_HDMI_EVIDENCE_SCHEMA)
			$fatal(1, "first transaction did not start at schema");
		close_command();
		read_evidence(lost_flags, 16'd3);

		// Exercise saturation without spending 65,535 simulated transitions.
		drive_synchronized_lock(1'b1);
		@(negedge clk_sys);
		dut.recorder.lock_loss_count = 16'hffff;
		dut.recorder.lock_ever_lost = 1'b1;
		drive_synchronized_lock(1'b0);
		read_evidence(lost_flags |
			MAGIK_HDMI_EVIDENCE_FLAG_LOCK_LOSS_COUNT_OVERFLOW, 16'hffff);

		$display("HDMI lock evidence tests passed");
		$finish;
	end
endmodule

`default_nettype wire
