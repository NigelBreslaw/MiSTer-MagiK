// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ns/1ps
`default_nettype none
module tb_mister_magik_crt_timing;
	reg clk_video=0, reset=1, test_pattern_enable=1;
	wire ce_pixel; wire [9:0] x; wire [8:0] y;
	wire hsync,vsync,de,hblank,vblank,frame_start,vblank_start;
	wire [7:0] test_r,test_g,test_b;
	integer ticks=0, active_ticks=0, hsync_ticks=0, vsync_ticks=0;
	integer frame_pulses=0, vblank_pulses=0, ce_gap=0; reg old_ce=0, seen_ce=0;
	mister_magik_crt_timing dut(.clk_video(clk_video),.reset(reset),
		.test_pattern_enable(test_pattern_enable),.ce_pixel(ce_pixel),.x(x),.y(y),
		.hsync(hsync),.vsync(vsync),.de(de),.hblank(hblank),.vblank(vblank),
		.frame_start(frame_start),.vblank_start(vblank_start),.test_r(test_r),
		.test_g(test_g),.test_b(test_b));
	always #5 clk_video=~clk_video;
	task automatic fail(input [8*96-1:0] message); begin
		$display("FAIL: %0s",message); $fatal(1); end endtask
	always @(negedge clk_video) if(!reset) begin
		if(ce_pixel && old_ce) fail("CE_PIXEL high on adjacent clocks");
		if(ce_pixel) begin
			if(seen_ce && ce_gap!=1) fail("CE_PIXEL is not divide-by-two");
			seen_ce=1; ce_gap=0; ticks=ticks+1;
			if(de) active_ticks=active_ticks+1;
			if(!hsync) hsync_ticks=hsync_ticks+1;
			if(!vsync) vsync_ticks=vsync_ticks+1;
			if(hblank !== (x>=640) || vblank !== (y>=240)) fail("blank mismatch");
			if(x==655 && !hsync) fail("horizontal sync starts early");
			if(x==656 && hsync) fail("horizontal sync did not start");
			if(x==751 && hsync) fail("horizontal sync ended early");
			if(x==752 && !hsync) fail("horizontal sync did not end");
			if(y==242 && !vsync) fail("vertical sync starts early");
			if(y==243 && vsync) fail("vertical sync did not start");
			if(y==245 && vsync) fail("vertical sync ended early");
			if(y==246 && !vsync) fail("vertical sync did not end");
		end else ce_gap=ce_gap+1;
		if(frame_start) frame_pulses=frame_pulses+1;
		if(vblank_start) vblank_pulses=vblank_pulses+1;
		old_ce=ce_pixel;
	end
	initial begin
		repeat(4) @(posedge clk_video); reset=0;
		wait(frame_pulses==1); @(negedge clk_video);
		if(ticks!=800*262) fail("incorrect pixel ticks per frame");
		if(active_ticks!=640*240) fail("incorrect active rectangle");
		if(hsync_ticks!=96*262) fail("incorrect horizontal sync width");
		if(vsync_ticks!=3*800) fail("incorrect vertical sync width");
		if(vblank_pulses!=1) fail("vertical blank pulse count mismatch");
		if(test_r==0 && test_g==0 && test_b==0) fail("test pattern black at origin");
		reset=1; repeat(2) @(posedge clk_video); @(negedge clk_video);
		if(x!=0 || y!=0 || ce_pixel) fail("reset did not clear raster");
		$display("PASS: 640x240p60 raster timing and pattern"); $finish;
	end
endmodule
`default_nettype wire
