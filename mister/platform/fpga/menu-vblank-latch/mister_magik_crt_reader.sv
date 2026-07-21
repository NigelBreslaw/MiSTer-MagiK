// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Linear RGB565 line reader for the two qualified scanout slots. The bounded
// burst state machine, reset draining policy, and rolling line-buffer ownership
// are adapted from MiSTer-devel/Arcade-BlackWidow_MiSTer videodr0me_fb RTL.
module mister_magik_crt_reader #(
	parameter integer TIMEOUT_CYCLES = 4096
) (
	input  wire        clk_sys,
	input  wire        reset,
	input  wire [31:0] frame_base,
	input  wire [11:0] frame_width,
	input  wire [11:0] frame_height,
	input  wire [13:0] frame_stride,
	input  wire        request_toggle,
	input  wire        request_bank,
	input  wire [8:0]  request_y,
	output reg         ready_toggle = 1'b0,
	output reg         ready_bank = 1'b0,
	output reg         reader_valid = 1'b0,
	output reg         fallback = 1'b1,
	output reg         timeout_error = 1'b0,
	output reg  [15:0] timeout_count = 16'd0,

	input  wire        video_bank,
	input  wire [9:0]  video_x,
	output wire [15:0] video_pixel,

	output wire        DDRAM_CLK,
	input  wire        DDRAM_BUSY,
	output reg  [7:0]  DDRAM_BURSTCNT = 8'd0,
	output reg  [28:0] DDRAM_ADDR = 29'd0,
	input  wire [63:0] DDRAM_DOUT,
	input  wire        DDRAM_DOUT_READY,
	output reg         DDRAM_RD = 1'b0,
	output wire [63:0] DDRAM_DIN,
	output wire [7:0]  DDRAM_BE,
	output wire        DDRAM_WE
);
	localparam [31:0] SLOT0_BASE = 32'h227e9000;
	localparam [31:0] SLOT1_BASE = 32'h22fd2000;
	localparam [2:0] IDLE=0, ISSUE0=1, READ0=2, ISSUE1=3, READ1=4, DRAIN=5;

	reg [2:0] state = IDLE;
	reg request_meta = 1'b0, request_sync = 1'b0, request_seen = 1'b0;
	reg active_bank = 1'b0;
	reg [8:0] active_y = 9'd0;
	reg [8:0] beat_index = 9'd0;
	reg [10:0] pixel_index = 11'd0;
	reg [15:0] timeout_timer = 16'd0;
	reg [8:0] drain_beats = 9'd0;
	reg [15:0] line0 [0:639];
	reg [15:0] line1 [0:639];

	assign DDRAM_CLK = clk_sys;
	assign DDRAM_DIN = 64'd0;
	assign DDRAM_BE = 8'd0;
	assign DDRAM_WE = 1'b0;
	assign video_pixel = video_x < 640 ? (video_bank ? line1[video_x] : line0[video_x]) : 16'd0;

	wire valid_geometry = (frame_width == 640) && (frame_height == 240) &&
	                      (frame_stride == 1280) &&
	                      ((frame_base == SLOT0_BASE) || (frame_base == SLOT1_BASE));
	wire timed_out = timeout_timer == TIMEOUT_CYCLES - 1;

	task automatic store_beat(input [63:0] beat);
		begin
			if(active_bank) begin
				line1[pixel_index]     <= beat[15:0];
				line1[pixel_index + 1] <= beat[31:16];
				line1[pixel_index + 2] <= beat[47:32];
				line1[pixel_index + 3] <= beat[63:48];
			end else begin
				line0[pixel_index]     <= beat[15:0];
				line0[pixel_index + 1] <= beat[31:16];
				line0[pixel_index + 2] <= beat[47:32];
				line0[pixel_index + 3] <= beat[63:48];
			end
		end
	endtask

	always @(posedge clk_sys) begin
		request_meta <= request_toggle;
		request_sync <= request_meta;
		DDRAM_RD <= 1'b0;

		if(reset) begin
			request_seen <= request_sync;
			ready_toggle <= 1'b0;
			reader_valid <= 1'b0;
			fallback <= 1'b1;
			timeout_error <= 1'b0;
			timeout_timer <= 16'd0;
			if((state == READ0) || (state == READ1)) begin
				state <= DRAIN;
				drain_beats <= (state == READ0) ? (9'd128 - beat_index) : (9'd32 - beat_index);
			end else state <= IDLE;
		end else begin
			case(state)
				IDLE: begin
					timeout_timer <= 16'd0;
					if(request_sync != request_seen) begin
						request_seen <= request_sync;
						ready_bank <= request_bank;
						active_bank <= request_bank;
						active_y <= request_y;
						timeout_error <= 1'b0;
						if(valid_geometry && request_y < 240) begin
							fallback <= 1'b1;
							reader_valid <= 1'b0;
							DDRAM_ADDR <= (frame_base + (request_y * 32'd1280)) >> 3;
							state <= ISSUE0;
						end else begin
							fallback <= 1'b1;
							reader_valid <= 1'b0;
							ready_toggle <= request_sync;
						end
					end
				end
				ISSUE0: begin
					if(!DDRAM_BUSY) begin
						DDRAM_BURSTCNT <= 8'd128;
						DDRAM_RD <= 1'b1;
						beat_index <= 9'd0;
						pixel_index <= 11'd0;
						timeout_timer <= 16'd0;
						state <= READ0;
					end else if(timed_out) begin
						timeout_error <= 1'b1; fallback <= 1'b1;
						if(timeout_count != 16'hffff) timeout_count <= timeout_count + 1'd1;
						ready_toggle <= request_seen; state <= IDLE;
					end else timeout_timer <= timeout_timer + 1'd1;
				end
				READ0: begin
					if(DDRAM_DOUT_READY) begin
						store_beat(DDRAM_DOUT);
						timeout_timer <= 16'd0;
						if(beat_index == 9'd127) begin
							DDRAM_ADDR <= DDRAM_ADDR + 9'd128;
							state <= ISSUE1;
						end else begin
							beat_index <= beat_index + 1'd1;
							pixel_index <= pixel_index + 3'd4;
						end
					end else if(timed_out) begin
						timeout_error <= 1'b1; fallback <= 1'b1;
						if(timeout_count != 16'hffff) timeout_count <= timeout_count + 1'd1;
						drain_beats <= 9'd128 - beat_index; state <= DRAIN;
					end else timeout_timer <= timeout_timer + 1'd1;
				end
				ISSUE1: begin
					if(!DDRAM_BUSY) begin
						DDRAM_BURSTCNT <= 8'd32;
						DDRAM_RD <= 1'b1;
						beat_index <= 9'd0;
						pixel_index <= 11'd512;
						timeout_timer <= 16'd0;
						state <= READ1;
					end else if(timed_out) begin
						timeout_error <= 1'b1; fallback <= 1'b1;
						if(timeout_count != 16'hffff) timeout_count <= timeout_count + 1'd1;
						ready_toggle <= request_seen; state <= IDLE;
					end else timeout_timer <= timeout_timer + 1'd1;
				end
				READ1: begin
					if(DDRAM_DOUT_READY) begin
						store_beat(DDRAM_DOUT);
						timeout_timer <= 16'd0;
						if(beat_index == 9'd31) begin
							reader_valid <= 1'b1; fallback <= 1'b0;
							ready_toggle <= request_seen; state <= IDLE;
						end else begin
							beat_index <= beat_index + 1'd1;
							pixel_index <= pixel_index + 3'd4;
						end
					end else if(timed_out) begin
						timeout_error <= 1'b1; fallback <= 1'b1;
						if(timeout_count != 16'hffff) timeout_count <= timeout_count + 1'd1;
						drain_beats <= 9'd32 - beat_index; state <= DRAIN;
					end else timeout_timer <= timeout_timer + 1'd1;
				end
				DRAIN: begin
					if(DDRAM_DOUT_READY && drain_beats != 0) drain_beats <= drain_beats - 1'd1;
					if(drain_beats == 0) begin ready_toggle <= request_seen; state <= IDLE; end
				end
				default: state <= IDLE;
			endcase
		end
	end
endmodule
`default_nettype wire
