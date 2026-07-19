// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module mister_magik_vblank_latch (
	input  wire        clk_sys,
	input  wire        hdmi_vbl,
	input  wire        cmd_start,
	input  wire        cmd_data,
	input  wire [7:0]  cmd_id,
	input  wire [3:0]  word_index,
	input  wire [15:0] data_in,

	input  wire        active_lfb_en,
	input  wire [31:0] active_lfb_base,
	input  wire [11:0] active_lfb_width,
	input  wire [11:0] active_lfb_height,
	input  wire [13:0] active_lfb_stride,

	output wire        response_valid,
	output reg  [15:0] response_data,
	output wire        apply,

	output reg         route_en = 1'b0,
	output reg         route_flt = 1'b0,
	output reg  [5:0]  route_fmt = 6'd0,
	output reg  [11:0] route_width = 12'd0,
	output reg  [11:0] route_height = 12'd0,
	output reg  [11:0] route_hmin = 12'd0,
	output reg  [11:0] route_hmax = 12'd0,
	output reg  [11:0] route_vmin = 12'd0,
	output reg  [11:0] route_vmax = 12'd0,
	output reg  [31:0] route_base = 32'd0,
	output reg  [13:0] route_stride = 14'd0,

	output reg         pending = 1'b0,
	output reg  [15:0] pending_seq = 16'd0,
	output reg  [15:0] active_seq = 16'd0,
	output reg  [15:0] post_count = 16'd0,
	output reg  [15:0] flip_count = 16'd0,
	output reg  [15:0] drop_count = 16'd0
);

	`include "mister_magik_latch_protocol.svh"

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg vbl_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg vbl_sys = 1'b0;
	reg vbl_old = 1'b0;

	wire vbl_rise = ~vbl_old & vbl_sys;
	assign apply = pending && vbl_rise;
	assign response_valid =
		(cmd_start && ((cmd_id == MAGIK_UIO_SET_FBUF_LATCH) ||
		               (cmd_id == MAGIK_UIO_GET_FBUF_LATCH) ||
		               (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_CAPS))) ||
		(cmd_data && ((cmd_id == MAGIK_UIO_GET_FBUF_LATCH) ||
		              (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_CAPS)));

	always @(*) begin
		response_data = 16'd0;
		if(cmd_start) begin
			case(cmd_id)
				MAGIK_UIO_SET_FBUF_LATCH: response_data = MAGIK_FBUF_LATCH_MAGIC;
				MAGIK_UIO_GET_FBUF_LATCH: response_data = MAGIK_FBUF_STATUS_MAGIC;
				MAGIK_UIO_GET_FBUF_LATCH_CAPS: response_data = MAGIK_FBUF_CAPS_MAGIC;
				default: response_data = 16'd0;
			endcase
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH)) begin
			case(word_index)
				4'd0:  response_data = active_seq;
				4'd1:  response_data = pending_seq;
				4'd2:  response_data = {13'd0, pending, route_en, active_lfb_en};
				4'd3:  response_data = flip_count;
				4'd4:  response_data = post_count;
				4'd5:  response_data = drop_count;
				4'd6:  response_data = active_lfb_base[15:0];
				4'd7:  response_data = active_lfb_base[31:16];
				4'd8:  response_data = {4'd0, active_lfb_width};
				4'd9:  response_data = {4'd0, active_lfb_height};
				4'd10: response_data = {2'd0, active_lfb_stride};
				default: response_data = 16'd0;
			endcase
		end
		else if(cmd_data && (cmd_id == MAGIK_UIO_GET_FBUF_LATCH_CAPS)) begin
			case(word_index)
				4'd0: response_data = MAGIK_FBUF_PROTOCOL_VERSION;
				4'd1: response_data = MAGIK_FBUF_CAPS_FLAGS;
				4'd2: response_data = MAGIK_FBUF_MAX_WIDTH;
				4'd3: response_data = MAGIK_FBUF_MAX_HEIGHT;
				4'd4: response_data = MAGIK_FBUF_MAX_STRIDE;
				default: response_data = 16'd0;
			endcase
		end
	end

	always @(posedge clk_sys) begin
		vbl_meta <= hdmi_vbl;
		vbl_sys <= vbl_meta;
		vbl_old <= vbl_sys;

		if(apply) begin
			active_seq <= pending_seq;
			flip_count <= flip_count + 1'd1;
			pending <= 1'b0;
		end

		if(cmd_data && (cmd_id == MAGIK_UIO_SET_FBUF_LATCH)) begin
			case(word_index)
				4'd0:  {route_en, route_flt, route_fmt} <=
					{data_in[15], data_in[14], data_in[5:0]};
				4'd1:  route_base[15:0] <= data_in;
				4'd2:  route_base[31:16] <= data_in;
				4'd3:  route_width <= data_in[11:0];
				4'd4:  route_height <= data_in[11:0];
				4'd5:  route_hmin <= data_in[11:0];
				4'd6:  route_hmax <= data_in[11:0];
				4'd7:  route_vmin <= data_in[11:0];
				4'd8:  route_vmax <= data_in[11:0];
				4'd9:  route_stride <= data_in[13:0];
				4'd10: begin
					pending_seq <= data_in;
					pending <= 1'b1;
					post_count <= post_count + 1'd1;
					if(pending) drop_count <= drop_count + 1'd1;
				end
				default: begin end
			endcase
		end
	end

endmodule

`default_nettype wire
