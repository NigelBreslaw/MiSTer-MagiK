// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// 640x240p60 native raster. CLK_VIDEO is 25.200 MHz; CE_PIXEL advances the
// raster every second clock for an effective 12.600 MHz pixel rate.
module mister_magik_crt_timing (
	input wire clk_video, input wire reset, input wire test_pattern_enable,
	output reg ce_pixel = 1'b0, output reg [9:0] x = 10'd0,
	output reg [8:0] y = 9'd0, output wire hsync, output wire vsync,
	output wire de, output wire hblank, output wire vblank,
	output reg frame_start = 1'b0, output reg vblank_start = 1'b0,
	output reg [7:0] test_r, output reg [7:0] test_g, output reg [7:0] test_b
);
	localparam integer H_ACTIVE=640, H_FRONT=16, H_SYNC=96, H_TOTAL=800;
	localparam integer V_ACTIVE=240, V_FRONT=3, V_SYNC=3, V_TOTAL=262;
	wire pixel_tick = ce_pixel;
	assign hblank = x >= H_ACTIVE;
	assign vblank = y >= V_ACTIVE;
	assign de = ~(hblank | vblank);
	assign hsync = ~((x >= H_ACTIVE+H_FRONT) && (x < H_ACTIVE+H_FRONT+H_SYNC));
	assign vsync = ~((y >= V_ACTIVE+V_FRONT) && (y < V_ACTIVE+V_FRONT+V_SYNC));

	always @(posedge clk_video) begin
		frame_start <= 1'b0;
		vblank_start <= 1'b0;
		if(reset) begin ce_pixel<=0; x<=0; y<=0; end
		else begin
			ce_pixel <= ~ce_pixel;
			if(pixel_tick) begin
				if(x == H_TOTAL-1) begin
					x <= 0;
					if(y == V_TOTAL-1) begin y<=0; frame_start<=1; end
					else begin y<=y+1'd1; if(y == V_ACTIVE-1) vblank_start<=1; end
				end else x <= x+1'd1;
			end
		end
	end

	always @(*) begin
		test_r=0; test_g=0; test_b=0;
		if(test_pattern_enable && de) begin
			case(x[9:7])
				0: begin test_r=8'hff; test_g=8'hff; test_b=8'hff; end
				1: begin test_r=8'hff; test_g=8'hff; test_b=0; end
				2: begin test_r=0; test_g=8'hff; test_b=8'hff; end
				3: begin test_r=0; test_g=8'hff; test_b=0; end
				4: begin test_r=8'hff; test_g=0; test_b=8'hff; end
				5: begin test_r=8'hff; test_g=0; test_b=0; end
				6: begin test_r=0; test_g=0; test_b=8'hff; end
				default: begin test_r=8'h20; test_g=8'h20; test_b=8'h20; end
			endcase
			if(x==0 || x==H_ACTIVE-1 || y==0 || y==V_ACTIVE-1 ||
			   x[5:0]==0 || y[5:0]==0 || x=={1'b0,y}) begin
				test_r=8'hff; test_g=8'hff; test_b=8'hff;
			end
		end
	end
endmodule
`default_nettype wire
