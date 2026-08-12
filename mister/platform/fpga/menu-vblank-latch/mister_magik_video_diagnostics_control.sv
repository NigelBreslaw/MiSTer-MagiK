// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Passive diagnostic recorder. Its only functional-system outputs are the
// response pair for the dedicated read-only opcodes.
module mister_magik_video_diagnostics_control #(
	parameter [23:0] HEARTBEAT_TIMEOUT_CYCLES = 24'd10000000,
	parameter [11:0] SNAPSHOT_TIMEOUT_CYCLES = 12'd4095
) (
	input  wire         clk_sys,
	input  wire         hdmi_vbl,
	input  wire         io_uio,
	input  wire         io_strobe,
	input  wire         io_osd,
	input  wire [15:0]  io_din,

	input  wire         apply_accepted,
	input  wire         pending,
	input  wire [15:0]  pending_seq,
	input  wire [15:0]  active_seq,
	input  wire [15:0]  post_count,
	input  wire [15:0]  active_route_epoch,
	input  wire         route_en,
	input  wire         route_flt,
	input  wire [5:0]   route_fmt,
	input  wire [11:0]  route_width,
	input  wire [11:0]  route_height,
	input  wire [11:0]  route_hmin,
	input  wire [11:0]  route_hmax,
	input  wire [11:0]  route_vmin,
	input  wire [11:0]  route_vmax,
	input  wire [31:0]  route_base,
	input  wire [13:0]  route_stride,

	input  wire         lfb_en,
	input  wire         lfb_flt,
	input  wire [5:0]   lfb_fmt,
	input  wire [11:0]  lfb_width,
	input  wire [11:0]  lfb_height,
	input  wire [11:0]  lfb_hmin,
	input  wire [11:0]  lfb_hmax,
	input  wire [11:0]  lfb_vmin,
	input  wire [11:0]  lfb_vmax,
	input  wire [31:0]  lfb_base,
	input  wire [13:0]  lfb_stride,

	input  wire         reset_req,
	input  wire         reset_out,
	input  wire         cfg_done,
	input  wire         hdmi_pll_locked,
	input  wire         output_heartbeat_toggle_async,

	input  wire         avalon_fault_toggle_async,
	input  wire [7:0]   avalon_trigger_async,
	input  wire         avalon_snapshot_ack_async,
	input  wire [239:0] avalon_snapshot_payload_async,
	input  wire         output_fault_toggle_async,
	input  wire [7:0]   output_trigger_async,
	input  wire         output_snapshot_ack_async,
	input  wire [239:0] output_snapshot_payload_async,

	output reg          snapshot_request_toggle = 1'b0,
	output wire         monitor_armed,
	output wire         hdmi_pll_locked_sync,
	output wire [15:0]  diagnostic_generation,
	output reg          route_context_toggle = 1'b0,
	output reg  [31:0]  expected_base = 32'd0,
	output reg  [15:0]  expected_route_epoch = 16'd0,
	output reg  [15:0]  expected_active_seq = 16'd0,
	output reg  [15:0]  expected_route_flags = 16'd0,

	output wire         response_valid,
	output reg  [15:0]  response_data
);

`include "mister_magik_video_diagnostics_protocol.svh"

	reg [7:0] command = 8'd0;
	reg       has_command = 1'b0;
	reg [7:0] word_count = 8'd0;
	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire [7:0] command_id = has_command ? command : io_din[7:0];
	wire [7:0] snapshot_select_command = command_start ? io_din[7:0] : command;
	wire [7:0] snapshot_select_word = command_start ? 8'd0 : (word_count + 1'd1);
	wire diagnostic_command =
		(command_id == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_CONTROL) ||
		(command_id == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_AVALON) ||
		(command_id == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_OUTPUT);
	wire [7:0] response_words =
		(command_id == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_CONTROL) ?
			MAGIK_VIDEO_DIAGNOSTICS_CONTROL_WORDS :
		(command_id == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_AVALON) ?
			MAGIK_VIDEO_DIAGNOSTICS_AVALON_WORDS :
			MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_WORDS;
	assign response_valid =
		(command_start && diagnostic_command) ||
		(command_data && diagnostic_command && (word_count < response_words));

	reg [1:0] state = MAGIK_VIDEO_DIAGNOSTICS_STATE_IDLE;
	reg [7:0] trigger = MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_NONE;
	reg [2:0] missing_domains = 3'b000;
	reg [15:0] generation = 16'd0;
	reg [15:0] vblank_count = 16'd0;
	reg [1:0] settle_vblanks = 2'd0;
	reg ownership = 1'b0;
	assign monitor_armed = state == MAGIK_VIDEO_DIAGNOSTICS_STATE_ARMED;
	assign diagnostic_generation = generation;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg avalon_fault_meta = 1'b0, avalon_fault_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_fault_meta = 1'b0, output_fault_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg avalon_ack_meta = 1'b0, avalon_ack_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg output_ack_meta = 1'b0, output_ack_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg heartbeat_meta = 1'b0, heartbeat_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg control_vbl_meta = 1'b0, control_vbl_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg control_reset_req_meta = 1'b0, control_reset_req_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg control_reset_out_meta = 1'b0, control_reset_out_sys = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg control_pll_lock_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg control_pll_lock_sys = 1'b0;
	assign hdmi_pll_locked_sync = control_pll_lock_sys;
	reg avalon_fault_seen = 1'b0, output_fault_seen = 1'b0;
	reg avalon_ack_seen = 1'b0, output_ack_seen = 1'b0;
	reg heartbeat_seen = 1'b0;
	reg [23:0] heartbeat_age = 24'd0;

	reg vbl_d = 1'b0;
	wire vblank_rise = control_vbl_sys && !vbl_d;

	reg legacy_open = 1'b0;
	reg legacy_owned_at_start = 1'b0;
	reg [9:0] legacy_mask = 10'd0;
	reg [15:0] legacy_words [0:9];
	reg [7:0] legacy_total = 8'd0;
	reg [7:0] legacy_owned = 8'd0;
	reg [7:0] legacy_partial = 8'd0;
	reg [7:0] legacy_abort = 8'd0;
	reg [3:0] legacy_disposition = MAGIK_VIDEO_DIAGNOSTICS_DISPOSITION_NONE;

	reg [31:0] pre_base = 32'd0, post_base = 32'd0;
	reg [4:0] pre_route_flags = 5'd0, post_route_flags = 5'd0;
	reg [11:0] pre_width = 12'd0, pre_height = 12'd0;
	reg [11:0] pre_hmin = 12'd0, pre_hmax = 12'd0;
	reg [11:0] pre_vmin = 12'd0, pre_vmax = 12'd0;
	reg [13:0] pre_stride = 14'd0, post_stride = 14'd0;
	reg [11:0] post_width = 12'd0, post_height = 12'd0;
	reg [7:0] control_fault_flags = 8'd0;
	reg [1:0] route_mismatch_vblanks = 2'd0;
	reg [11:0] freeze_timeout = 12'd0;
	reg freeze_pending = 1'b0;
	reg [15:0] frozen_vblank_count = 16'd0;
	reg [15:0] frozen_active_seq = 16'd0, frozen_post_count = 16'd0;
	reg [15:0] frozen_active_route_epoch = 16'd0;
	reg [4:0] frozen_route_state_flags = 5'd0;

	reg avalon_sample_pending = 1'b0, output_sample_pending = 1'b0;
	reg [15:0] avalon_verify_sample = 16'd0, output_verify_sample = 16'd0;
	reg avalon_trigger_sample_pending = 1'b0, output_trigger_sample_pending = 1'b0;
	reg [7:0] avalon_trigger_sample = 8'd0, output_trigger_sample = 8'd0;
	integer capture_index;

	reg [15:0] tx_crc = 16'hffff;
	reg [7:0] tx_command = 8'd0;
	reg [15:0] response_word = 16'd0;
	reg freeze_request_now;
	reg [7:0] freeze_request_trigger;
	reg [7:0] freeze_request_flags;

	initial begin
		for(capture_index = 0; capture_index < 10; capture_index = capture_index + 1)
			legacy_words[capture_index] = 16'd0;
	end

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
			crc_update_word = crc_update_byte(crc_update_byte(crc_in, word_in[15:8]), word_in[7:0]);
		end
	endfunction

	function automatic [15:0] crc_header;
		input [7:0] header_command;
		input [15:0] payload_words;
		reg [15:0] value;
		begin
			value = crc_update_word(16'hffff, {8'd0, header_command});
			value = crc_update_word(value, MAGIK_VIDEO_DIAGNOSTICS_SCHEMA);
			crc_header = crc_update_word(value, payload_words);
		end
	endfunction

	function automatic [15:0] state_flags;
		begin
			state_flags = {10'd0,
				(state == MAGIK_VIDEO_DIAGNOSTICS_STATE_PARTIAL),
				1'b0,
				(state == MAGIK_VIDEO_DIAGNOSTICS_STATE_FROZEN) ||
					(state == MAGIK_VIDEO_DIAGNOSTICS_STATE_PARTIAL),
				monitor_armed, state};
		end
	endfunction

	// Keep the control selector explicit. Quartus 17 otherwise treats state
	// referenced only through nested automatic functions as unused and can
	// optimize diagnostic evidence out of the response path.
	reg [15:0] current_snapshot_word;
	always @(*) begin
		current_snapshot_word = 16'd0;
		case(snapshot_select_command)
			MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_CONTROL: begin
				case(snapshot_select_word)
					0: current_snapshot_word = MAGIK_VIDEO_DIAGNOSTICS_SCHEMA;
					1: current_snapshot_word = state_flags();
					2: current_snapshot_word = {8'd0, trigger};
					3: current_snapshot_word = {13'd0, missing_domains};
					4: current_snapshot_word = generation;
					5: current_snapshot_word = 16'd1;
					6: current_snapshot_word = frozen_vblank_count;
					7: current_snapshot_word = {legacy_total, legacy_owned};
					8: current_snapshot_word = {legacy_partial, legacy_abort};
					9: current_snapshot_word = {legacy_disposition, 2'd0, legacy_mask};
					10: current_snapshot_word = {control_fault_flags, 3'd0,
						frozen_route_state_flags};
					11: current_snapshot_word = frozen_active_seq;
					12: current_snapshot_word = frozen_post_count;
					13: current_snapshot_word = frozen_active_route_epoch;
					14: current_snapshot_word = legacy_words[0];
					15: current_snapshot_word = legacy_words[1];
					16: current_snapshot_word = legacy_words[2];
					17: current_snapshot_word = legacy_words[3];
					18: current_snapshot_word = legacy_words[4];
					19: current_snapshot_word = legacy_words[5];
					20: current_snapshot_word = legacy_words[6];
					21: current_snapshot_word = legacy_words[7];
					22: current_snapshot_word = legacy_words[8];
					23: current_snapshot_word = legacy_words[9];
					24: current_snapshot_word = {11'd0, pre_route_flags};
					25: current_snapshot_word = pre_base[15:0];
					26: current_snapshot_word = pre_base[31:16];
					27: current_snapshot_word = pre_width;
					28: current_snapshot_word = pre_height;
					29: current_snapshot_word = pre_hmin;
					30: current_snapshot_word = pre_hmax;
					31: current_snapshot_word = pre_vmin;
					32: current_snapshot_word = pre_vmax;
					33: current_snapshot_word = {2'd0, pre_stride};
					34: current_snapshot_word = post_base[15:0];
					35: current_snapshot_word = post_base[31:16];
					36: current_snapshot_word = {11'd0, post_route_flags};
					37: current_snapshot_word = post_width;
					38: current_snapshot_word = post_height;
					39: current_snapshot_word = {2'd0, post_stride};
					default: current_snapshot_word = 16'd0;
				endcase
			end
			MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_AVALON: begin
				case(snapshot_select_word)
					0: current_snapshot_word = avalon_snapshot_payload_async[15:0];
					1: current_snapshot_word = avalon_snapshot_payload_async[31:16];
					2: current_snapshot_word = avalon_snapshot_payload_async[47:32];
					3: current_snapshot_word = avalon_snapshot_payload_async[63:48];
					4: current_snapshot_word = avalon_snapshot_payload_async[79:64];
					5: current_snapshot_word = avalon_snapshot_payload_async[95:80];
					6: current_snapshot_word = avalon_snapshot_payload_async[111:96];
					7: current_snapshot_word = avalon_snapshot_payload_async[127:112];
					8: current_snapshot_word = avalon_snapshot_payload_async[143:128];
					9: current_snapshot_word = avalon_snapshot_payload_async[159:144];
					10: current_snapshot_word = avalon_snapshot_payload_async[175:160];
					11: current_snapshot_word = avalon_snapshot_payload_async[191:176];
					12: current_snapshot_word = avalon_snapshot_payload_async[207:192];
					13: current_snapshot_word = avalon_snapshot_payload_async[223:208];
					14: current_snapshot_word = avalon_snapshot_payload_async[239:224];
					default: current_snapshot_word = 16'd0;
				endcase
			end
			MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_OUTPUT: begin
				case(snapshot_select_word)
					0: current_snapshot_word = output_snapshot_payload_async[15:0];
					1: current_snapshot_word = output_snapshot_payload_async[31:16];
					2: current_snapshot_word = output_snapshot_payload_async[47:32];
					3: current_snapshot_word = output_snapshot_payload_async[63:48];
					4: current_snapshot_word = output_snapshot_payload_async[79:64];
					5: current_snapshot_word = output_snapshot_payload_async[95:80];
					6: current_snapshot_word = output_snapshot_payload_async[111:96];
					7: current_snapshot_word = output_snapshot_payload_async[127:112];
					8: current_snapshot_word = output_snapshot_payload_async[143:128];
					9: current_snapshot_word = output_snapshot_payload_async[159:144];
					10: current_snapshot_word = output_snapshot_payload_async[175:160];
					11: current_snapshot_word = output_snapshot_payload_async[191:176];
					12: current_snapshot_word = output_snapshot_payload_async[207:192];
					13: current_snapshot_word = output_snapshot_payload_async[223:208];
					14: current_snapshot_word = output_snapshot_payload_async[239:224];
					default: current_snapshot_word = 16'd0;
				endcase
			end
			default: current_snapshot_word = 16'd0;
		endcase
	end

	always @(*) begin
		response_data = 16'd0;
		if(command_start && diagnostic_command) begin
			case(command_id)
				MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_CONTROL:
					response_data = MAGIK_VIDEO_DIAGNOSTICS_CONTROL_MAGIC;
				MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_AVALON:
					response_data = MAGIK_VIDEO_DIAGNOSTICS_AVALON_MAGIC;
				MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_OUTPUT:
					response_data = MAGIK_VIDEO_DIAGNOSTICS_OUTPUT_MAGIC;
				default: response_data = 16'd0;
			endcase
		end
		else if(command_data && diagnostic_command) begin
			if(word_count == (response_words - 1'd1)) response_data = tx_crc;
			else response_data = response_word;
		end
	end

	task automatic request_freeze;
		input [7:0] fault_trigger;
		input [7:0] fault_flags;
		begin
			if((state == MAGIK_VIDEO_DIAGNOSTICS_STATE_ARMED) &&
			   !freeze_pending && !freeze_request_now) begin
				freeze_request_now = 1'b1;
				freeze_request_trigger = fault_trigger;
				freeze_request_flags = fault_flags;
			end
		end
	endtask

	always @(posedge clk_sys) begin
		freeze_request_now = 1'b0;
		freeze_request_trigger = MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_NONE;
		freeze_request_flags = 8'd0;
		vbl_d <= control_vbl_sys;
		avalon_fault_meta <= avalon_fault_toggle_async;
		avalon_fault_sys <= avalon_fault_meta;
		output_fault_meta <= output_fault_toggle_async;
		output_fault_sys <= output_fault_meta;
		avalon_ack_meta <= avalon_snapshot_ack_async;
		avalon_ack_sys <= avalon_ack_meta;
		output_ack_meta <= output_snapshot_ack_async;
		output_ack_sys <= output_ack_meta;
		heartbeat_meta <= output_heartbeat_toggle_async;
		heartbeat_sys <= heartbeat_meta;
		control_vbl_meta <= hdmi_vbl;
		control_vbl_sys <= control_vbl_meta;
		control_reset_req_meta <= reset_req;
		control_reset_req_sys <= control_reset_req_meta;
		control_reset_out_meta <= reset_out;
		control_reset_out_sys <= control_reset_out_meta;
		control_pll_lock_meta <= hdmi_pll_locked;
		control_pll_lock_sys <= control_pll_lock_meta;

		if(heartbeat_sys != heartbeat_seen) begin
			heartbeat_seen <= heartbeat_sys;
			heartbeat_age <= 24'd0;
		end
		else if(monitor_armed && ownership && lfb_en && heartbeat_age < HEARTBEAT_TIMEOUT_CYCLES)
			heartbeat_age <= heartbeat_age + 1'd1;

		if(vblank_rise) begin
			vblank_count <= vblank_count + 1'd1;
			if(ownership && settle_vblanks < 2) settle_vblanks <= settle_vblanks + 1'd1;
			if(ownership && settle_vblanks == 1 && state == MAGIK_VIDEO_DIAGNOSTICS_STATE_IDLE)
				state <= MAGIK_VIDEO_DIAGNOSTICS_STATE_ARMED;
			if(monitor_armed) begin
				if({lfb_en,lfb_flt,lfb_fmt,lfb_width,lfb_height,lfb_hmin,lfb_hmax,
					lfb_vmin,lfb_vmax,lfb_base,lfb_stride} !=
					{route_en,route_flt,route_fmt,route_width,route_height,route_hmin,
					route_hmax,route_vmin,route_vmax,route_base,route_stride}) begin
					if(route_mismatch_vblanks == 1)
						request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_ROUTE_DIVERGENCE,
							MAGIK_VIDEO_DIAGNOSTICS_CONTROL_FAULT_FLAGS_ROUTE_DIVERGENCE);
					else route_mismatch_vblanks <= route_mismatch_vblanks + 1'd1;
				end
				else route_mismatch_vblanks <= 2'd0;
			end
		end

		if(apply_accepted && !freeze_pending &&
		   (state != MAGIK_VIDEO_DIAGNOSTICS_STATE_FROZEN) &&
		   (state != MAGIK_VIDEO_DIAGNOSTICS_STATE_PARTIAL)) begin
			ownership <= 1'b1;
			settle_vblanks <= 2'd0;
			expected_base <= route_base;
			expected_route_epoch <= active_route_epoch + 1'd1;
			expected_active_seq <= pending_seq;
			expected_route_flags <= {11'd0, route_flt, 1'b1, route_en, 1'b0, 1'b1};
			route_context_toggle <= ~route_context_toggle;
		end

		if(command_start) begin
			command <= io_din[7:0];
			has_command <= 1'b1;
			word_count <= 8'd0;
			if(io_din[7:0] == 8'h57) begin
				// Observer-only transaction count is supplied by post_count.
			end
			if((io_din[7:0] == 8'h2f) && !freeze_pending &&
			   (state != MAGIK_VIDEO_DIAGNOSTICS_STATE_FROZEN) &&
			   (state != MAGIK_VIDEO_DIAGNOSTICS_STATE_PARTIAL)) begin
				legacy_open <= 1'b1;
				legacy_mask <= 10'd0;
				legacy_owned_at_start <= ownership;
				pre_base <= lfb_base;
				pre_route_flags <= {lfb_flt, 1'b0, lfb_en, pending, ownership};
				pre_width <= lfb_width;
				pre_height <= lfb_height;
				pre_hmin <= lfb_hmin;
				pre_hmax <= lfb_hmax;
				pre_vmin <= lfb_vmin;
				pre_vmax <= lfb_vmax;
				pre_stride <= lfb_stride;
			end
			if((io_din[7:0] == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_CONTROL) ||
			   (io_din[7:0] == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_AVALON) ||
			   (io_din[7:0] == MAGIK_UIO_GET_VIDEO_DIAGNOSTICS_OUTPUT)) begin
				tx_command <= io_din[7:0];
				tx_crc <= crc_header(io_din[7:0], response_words - 1'd1);
				response_word <= current_snapshot_word;
			end
		end
		else if(command_data) begin
			word_count <= word_count + 1'd1;
			if((command == 8'h2f) && legacy_open && (word_count < 10)) begin
				case(word_count)
					0: legacy_words[0] <= io_din;
					1: legacy_words[1] <= io_din;
					2: legacy_words[2] <= io_din;
					3: legacy_words[3] <= io_din;
					4: legacy_words[4] <= io_din;
					5: legacy_words[5] <= io_din;
					6: legacy_words[6] <= io_din;
					7: legacy_words[7] <= io_din;
					8: legacy_words[8] <= io_din;
					9: legacy_words[9] <= io_din;
					default: begin end
				endcase
				legacy_mask[word_count] <= 1'b1;
				ownership <= 1'b0;
			end
			if((command == tx_command) && diagnostic_command &&
			   (word_count < (response_words - 1'd1)))
				tx_crc <= crc_update_word(tx_crc, response_word);
			if(diagnostic_command && (word_count < (response_words - 2'd2)))
				response_word <= current_snapshot_word;
		end

		if(!io_uio && has_command) begin
			has_command <= 1'b0;
			command <= 8'd0;
			word_count <= 8'd0;
			response_word <= 16'd0;
			if(legacy_open) begin
				legacy_open <= 1'b0;
				if(legacy_total != 8'hff) legacy_total <= legacy_total + 1'd1;
				post_base <= lfb_base;
				post_route_flags <= {lfb_flt, 1'b0, lfb_en, pending, ownership};
				post_width <= lfb_width;
				post_height <= lfb_height;
				post_stride <= lfb_stride;
				if(legacy_mask == 10'h3ff)
					legacy_disposition <= MAGIK_VIDEO_DIAGNOSTICS_DISPOSITION_COMPLETE;
				else begin
					legacy_disposition <= MAGIK_VIDEO_DIAGNOSTICS_DISPOSITION_PARTIAL;
					if(legacy_partial != 8'hff) legacy_partial <= legacy_partial + 1'd1;
					if(legacy_abort != 8'hff) legacy_abort <= legacy_abort + 1'd1;
				end
				if(legacy_owned_at_start) begin
					if(legacy_owned != 8'hff) legacy_owned <= legacy_owned + 1'd1;
					request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_LEGACY_OWNED, 8'd0);
				end
			end
		end

		if(monitor_armed && io_osd && io_strobe)
			request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_OWNED_OSD_WRITE,
				MAGIK_VIDEO_DIAGNOSTICS_CONTROL_FAULT_FLAGS_OWNED_OSD_WRITE);
		if(monitor_armed &&
		   (control_reset_req_sys || control_reset_out_sys ||
		    !cfg_done || !control_pll_lock_sys))
			request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_CONTROL_OR_CLOCK,
				{8'd0, heartbeat_age == HEARTBEAT_TIMEOUT_CYCLES, 2'd0,
				 !control_pll_lock_sys, !cfg_done,
				 control_reset_out_sys, control_reset_req_sys});
		if(monitor_armed && heartbeat_age == HEARTBEAT_TIMEOUT_CYCLES)
			request_freeze(MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_CONTROL_OR_CLOCK,
				MAGIK_VIDEO_DIAGNOSTICS_CONTROL_FAULT_FLAGS_HEARTBEAT_TIMEOUT);

		if(monitor_armed && (avalon_fault_sys != avalon_fault_seen)) begin
			avalon_fault_seen <= avalon_fault_sys;
			avalon_trigger_sample <= avalon_trigger_async;
			avalon_trigger_sample_pending <= 1'b1;
		end
		if(monitor_armed && (output_fault_sys != output_fault_seen)) begin
			output_fault_seen <= output_fault_sys;
			output_trigger_sample <= output_trigger_async;
			output_trigger_sample_pending <= 1'b1;
		end
		if(avalon_trigger_sample_pending) begin
			avalon_trigger_sample_pending <= 1'b0;
			request_freeze(avalon_trigger_sample, 8'd0);
		end
		if(output_trigger_sample_pending) begin
			output_trigger_sample_pending <= 1'b0;
			request_freeze(output_trigger_sample, 8'd0);
		end

		// Commit at one site after priority arbitration. Keeping the wide frozen
		// context behind one enable prevents Quartus from building an eight-way
		// mux for every evidence bit.
		if(freeze_request_now) begin
			trigger <= freeze_request_trigger;
			generation <= generation + 1'd1;
			frozen_vblank_count <= vblank_count;
			frozen_route_state_flags <= apply_accepted ?
				{route_flt, 1'b1, route_en, 1'b0, 1'b1} :
				{route_flt, 1'b0, route_en, pending, ownership};
			frozen_active_seq <= apply_accepted ? pending_seq : active_seq;
			frozen_post_count <= post_count;
			frozen_active_route_epoch <= apply_accepted ?
				(active_route_epoch + 1'd1) : active_route_epoch;
			legacy_open <= 1'b0;
			control_fault_flags <= control_fault_flags | freeze_request_flags;
			missing_domains <= 3'b110;
			freeze_timeout <= 12'd0;
			freeze_pending <= 1'b1;
			snapshot_request_toggle <= ~snapshot_request_toggle;
		end

		if(freeze_pending) begin
			freeze_timeout <= freeze_timeout + 1'd1;
			if(avalon_ack_sys != avalon_ack_seen) begin
				avalon_ack_seen <= avalon_ack_sys;
				avalon_verify_sample <= avalon_snapshot_payload_async[63:48];
				avalon_sample_pending <= 1'b1;
			end
			if(output_ack_sys != output_ack_seen) begin
				output_ack_seen <= output_ack_sys;
				output_verify_sample <= output_snapshot_payload_async[63:48];
				output_sample_pending <= 1'b1;
			end
			if(avalon_sample_pending) begin
				if(avalon_verify_sample == generation) begin
					avalon_sample_pending <= 1'b0;
					missing_domains[1] <= 1'b0;
				end
				else avalon_verify_sample <= avalon_snapshot_payload_async[63:48];
			end
			if(output_sample_pending) begin
				if(output_verify_sample == generation) begin
					output_sample_pending <= 1'b0;
					missing_domains[2] <= 1'b0;
				end
				else output_verify_sample <= output_snapshot_payload_async[63:48];
			end
			if((missing_domains & 3'b110) == 0) begin
				freeze_pending <= 1'b0;
				state <= MAGIK_VIDEO_DIAGNOSTICS_STATE_FROZEN;
			end
			else if(freeze_timeout == SNAPSHOT_TIMEOUT_CYCLES) begin
				freeze_pending <= 1'b0;
				state <= MAGIK_VIDEO_DIAGNOSTICS_STATE_PARTIAL;
			end
		end
	end

endmodule

`default_nettype wire
