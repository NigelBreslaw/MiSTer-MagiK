// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Production adapter between Main's UIO word stream and the latch protocol.
// Its independent command counter follows the same io_uio/io_strobe framing as
// sys_top, so the exact adapter driven in simulation is the one built into RBFs.
module mister_magik_latch_sys_top_bridge (
	input  wire        clk_sys,
	input  wire        hdmi_vbl,
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	input  wire [15:0] evidence_word,

	input  wire        active_lfb_en,
	input  wire [31:0] active_lfb_base,
	input  wire [11:0] active_lfb_width,
	input  wire [11:0] active_lfb_height,
	input  wire [13:0] active_lfb_stride,

	output wire        response_valid,
	output wire [15:0] response_data,
	output wire        apply,
	output wire        apply_accepted,
	output wire        legacy_write,
	output wire [3:0]  active_word_index,
	output wire        evidence_valid,
	output wire [2:0]  evidence_selector,
	output wire        evidence_snapshot,

	output wire        route_en,
	output wire        route_flt,
	output wire [5:0]  route_fmt,
	output wire [11:0] route_width,
	output wire [11:0] route_height,
	output wire [11:0] route_hmin,
	output wire [11:0] route_hmax,
	output wire [11:0] route_vmin,
	output wire [11:0] route_vmax,
	output wire [31:0] route_base,
	output wire [13:0] route_stride,

	output wire        pending,
	output wire [15:0] pending_seq,
	output wire [15:0] active_seq,
	output wire [15:0] post_count,
	output wire [15:0] flip_count,
	output wire [15:0] drop_count,
	output wire [15:0] reject_count,
	output wire [15:0] active_route_epoch
);

	reg [7:0] command = 8'd0;
	reg has_command = 1'b0;
	reg [7:0] word_count = 8'd0;

	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire [7:0] command_id = has_command ? command : io_din[7:0];
	assign active_word_index = word_count[3:0];
	assign evidence_selector = command_id[2:0];
	assign evidence_valid = (command_id[7:3] == 5'b01100) &&
		(evidence_selector != 3'b111);
	assign evidence_snapshot = command_start && evidence_valid;
	assign legacy_write =
		command_data && (command == 8'h2f) && (word_count < 8'd10);
	assign apply_accepted = apply && !legacy_write;

	always @(posedge clk_sys) begin
		if(!io_uio) begin
			command <= 8'd0;
			has_command <= 1'b0;
			word_count <= 8'd0;
		end
		else if(io_strobe) begin
			if(!has_command) begin
				command <= io_din[7:0];
				has_command <= 1'b1;
				word_count <= 8'd0;
			end
			else begin
				word_count <= word_count + 1'd1;
			end
		end
	end

	mister_magik_vblank_latch latch (
		.clk_sys(clk_sys),
		.hdmi_vbl(hdmi_vbl),
		.cmd_start(command_start),
		.cmd_data(command_data),
		.cmd_id(command_id),
		.word_index(active_word_index),
		.data_in(io_din),
		.evidence_word(evidence_word),
		.active_lfb_en(active_lfb_en),
		.active_lfb_base(active_lfb_base),
		.active_lfb_width(active_lfb_width),
		.active_lfb_height(active_lfb_height),
		.active_lfb_stride(active_lfb_stride),
		.apply_accepted(apply_accepted),
		.legacy_write(legacy_write),
		.response_valid(response_valid),
		.response_data(response_data),
		.apply(apply),
		.route_en(route_en),
		.route_flt(route_flt),
		.route_fmt(route_fmt),
		.route_width(route_width),
		.route_height(route_height),
		.route_hmin(route_hmin),
		.route_hmax(route_hmax),
		.route_vmin(route_vmin),
		.route_vmax(route_vmax),
		.route_base(route_base),
		.route_stride(route_stride),
		.pending(pending),
		.pending_seq(pending_seq),
		.active_seq(active_seq),
		.post_count(post_count),
		.flip_count(flip_count),
		.drop_count(drop_count),
		.reject_count(reject_count),
		.active_route_epoch(active_route_epoch)
	);

endmodule

`default_nettype wire
