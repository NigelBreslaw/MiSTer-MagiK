// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ns/1ps
`default_nettype none
module tb_mister_magik_crt_reader;
	reg clk=0, reset=1, request_toggle=0, request_bank=0, video_bank=0;
	reg [8:0] request_y=0; reg [9:0] video_x=0;
	reg [31:0] frame_base=32'h227e9000; reg [11:0] width=640,height=240;
	reg [13:0] stride=1280; reg busy=0, dout_ready=0; reg [63:0] dout=0;
	wire ready_toggle,ready_bank,reader_valid,fallback,timeout_error;
	wire [15:0] timeout_count,video_pixel; wire ddram_clk,rd,we;
	wire [7:0] burst,be; wire [28:0] addr; wire [63:0] din;
	integer bursts=0, beats_left=0, delay_count=0;
	reg [15:0] seed=0;
	mister_magik_crt_reader #(.TIMEOUT_CYCLES(12)) dut(
		.clk_sys(clk),.reset(reset),.frame_base(frame_base),.frame_width(width),
		.frame_height(height),.frame_stride(stride),.request_toggle(request_toggle),
		.request_bank(request_bank),.request_y(request_y),.ready_toggle(ready_toggle),
		.ready_bank(ready_bank),.reader_valid(reader_valid),.fallback(fallback),
		.timeout_error(timeout_error),.timeout_count(timeout_count),.video_bank(video_bank),
		.video_x(video_x),.video_pixel(video_pixel),.DDRAM_CLK(ddram_clk),.DDRAM_BUSY(busy),
		.DDRAM_BURSTCNT(burst),.DDRAM_ADDR(addr),.DDRAM_DOUT(dout),
		.DDRAM_DOUT_READY(dout_ready),.DDRAM_RD(rd),.DDRAM_DIN(din),.DDRAM_BE(be),.DDRAM_WE(we));
	always #5 clk=~clk;
	task automatic fail(input [8*96-1:0] m); begin $display("FAIL: %0s",m); $fatal(1); end endtask
	always @(posedge clk) begin
		dout_ready<=0;
		if(rd) begin bursts=bursts+1; beats_left=burst; delay_count=2; seed=seed+1000; end
		else if(delay_count>0) delay_count=delay_count-1;
		else if(beats_left>0) begin
			dout<={seed+16'd3,seed+16'd2,seed+16'd1,seed}; seed=seed+16'd4;
			dout_ready<=1; beats_left=beats_left-1;
		end
	end
	task automatic request_line(input bank,input [8:0] line);
		begin request_bank=bank; request_y=line; request_toggle=~request_toggle;
			wait(ready_toggle==request_toggle); @(posedge clk); end
	endtask
	initial begin
		repeat(4) @(posedge clk); reset=0; busy=1; repeat(3) @(posedge clk); busy=0;
		request_line(0,0);
		if(!reader_valid || fallback || timeout_error) fail("valid line did not complete");
		if(bursts!=2) fail("line did not use two bursts");
		video_bank=0; video_x=0; #1; if(video_pixel!=1000) fail("first pixel mismatch");
		video_x=639; #1; if(video_pixel!=2639) fail("last pixel mismatch");
		frame_base=32'h22fd2000; request_line(1,239);
		if(!reader_valid || !ready_bank || bursts!=4) fail("second slot/bank failed");
		width=639; request_line(0,0);
		if(reader_valid || !fallback || bursts!=4) fail("invalid geometry issued DDR read");
		width=640; busy=1; request_line(0,1); busy=0;
		if(!timeout_error || timeout_count!=1 || !fallback) fail("busy timeout not reported");
		request_toggle=~request_toggle; request_y=2; wait(dut.state==3'd2);
		reset=1; repeat(2) @(posedge clk); reset=0; beats_left=0; dout_ready=0;
		repeat(4) @(posedge clk);
		if(reader_valid || !fallback || rd || we) fail("reset-in-flight fallback failed");
		if(ddram_clk!==clk || din!=0 || be!=0) fail("DDR output constants invalid");
		$display("PASS: RGB565 CRT DDR line reader"); $finish;
	end
endmodule
`default_nettype wire
