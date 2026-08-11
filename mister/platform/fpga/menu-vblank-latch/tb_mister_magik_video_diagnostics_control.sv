// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_control;
	reg clk_sys = 1'b0;
	always #5 clk_sys = ~clk_sys;

	reg hdmi_vbl = 1'b0;
	reg io_uio = 1'b0;
	reg io_strobe = 1'b0;
	reg io_osd = 1'b0;
	reg [15:0] io_din = 16'd0;
	reg apply_accepted = 1'b0;
	reg pending = 1'b0;
	reg [15:0] pending_seq = 16'd12;
	reg [15:0] active_seq = 16'd0;
	reg [15:0] post_count = 16'd0;
	reg [15:0] active_route_epoch = 16'd11;
	reg route_en = 1'b1;
	reg route_flt = 1'b0;
	reg [5:0] route_fmt = 6'd4;
	reg [11:0] route_width = 12'd960;
	reg [11:0] route_height = 12'd540;
	reg [11:0] route_hmin = 12'd0;
	reg [11:0] route_hmax = 12'd959;
	reg [11:0] route_vmin = 12'd0;
	reg [11:0] route_vmax = 12'd539;
	reg [31:0] route_base = 32'h227e9000;
	reg [13:0] route_stride = 14'd1920;
	reg lfb_en = 1'b1;
	reg lfb_flt = 1'b0;
	reg [5:0] lfb_fmt = 6'd4;
	reg [11:0] lfb_width = 12'd960;
	reg [11:0] lfb_height = 12'd540;
	reg [11:0] lfb_hmin = 12'd0;
	reg [11:0] lfb_hmax = 12'd959;
	reg [11:0] lfb_vmin = 12'd0;
	reg [11:0] lfb_vmax = 12'd539;
	reg [31:0] lfb_base = 32'h227e9000;
	reg [13:0] lfb_stride = 14'd1920;
	reg reset_req = 1'b0;
	reg reset_out = 1'b0;
	reg cfg_done = 1'b1;
	reg pll_adjust_locked = 1'b1;
	reg output_heartbeat = 1'b0;
	reg avalon_fault = 1'b0;
	reg output_fault = 1'b0;
	reg [495:0] avalon_payload = 496'd0;
	reg [495:0] output_payload = 496'd0;
	wire snapshot_request;
	wire monitor_armed;
	wire [15:0] diagnostic_generation;
	wire response_valid;
	wire [15:0] response_data;
	wire route_context_toggle;
	wire [31:0] expected_base, expected_slot_end;
	wire [15:0] expected_route_epoch, expected_active_seq, expected_route_flags;

	mister_magik_video_diagnostics_control dut (
		.clk_sys(clk_sys), .hdmi_vbl(hdmi_vbl), .io_uio(io_uio),
		.io_strobe(io_strobe), .io_osd(io_osd), .io_din(io_din),
		.apply_accepted(apply_accepted), .pending(pending), .pending_seq(pending_seq),
		.active_seq(active_seq),
		.post_count(post_count), .active_route_epoch(active_route_epoch),
		.route_en(route_en), .route_flt(route_flt), .route_fmt(route_fmt),
		.route_width(route_width), .route_height(route_height), .route_hmin(route_hmin),
		.route_hmax(route_hmax), .route_vmin(route_vmin), .route_vmax(route_vmax),
		.route_base(route_base), .route_stride(route_stride), .lfb_en(lfb_en),
		.lfb_flt(lfb_flt), .lfb_fmt(lfb_fmt), .lfb_width(lfb_width),
		.lfb_height(lfb_height), .lfb_hmin(lfb_hmin), .lfb_hmax(lfb_hmax),
		.lfb_vmin(lfb_vmin), .lfb_vmax(lfb_vmax), .lfb_base(lfb_base),
		.lfb_stride(lfb_stride), .reset_req(reset_req), .reset_out(reset_out),
		.cfg_done(cfg_done), .pll_adjust_locked(pll_adjust_locked),
		.output_heartbeat_toggle_async(output_heartbeat),
		.avalon_fault_toggle_async(avalon_fault), .avalon_trigger_async(8'd0),
		.avalon_snapshot_ack_async(snapshot_request),
		.avalon_snapshot_payload_async(avalon_payload),
		.output_fault_toggle_async(output_fault), .output_trigger_async(8'd0),
		.output_snapshot_ack_async(snapshot_request),
		.output_snapshot_payload_async(output_payload),
		.snapshot_request_toggle(snapshot_request), .monitor_armed(monitor_armed),
		.diagnostic_generation(diagnostic_generation),
		.route_context_toggle(route_context_toggle), .expected_base(expected_base),
		.expected_slot_end(expected_slot_end), .expected_route_epoch(expected_route_epoch),
		.expected_active_seq(expected_active_seq), .expected_route_flags(expected_route_flags),
		.response_valid(response_valid), .response_data(response_data)
	);

	function automatic [15:0] crc_byte;
		input [15:0] crc_in;
		input [7:0] data;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {data,8'd0};
			for(bit_index=0; bit_index<8; bit_index=bit_index+1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : value << 1;
			crc_byte = value;
		end
	endfunction

	function automatic [15:0] crc_word;
		input [15:0] crc_in;
		input [15:0] data;
		begin crc_word = crc_byte(crc_byte(crc_in,data[15:8]),data[7:0]); end
	endfunction

	task automatic strobe_word;
		input [15:0] value;
		begin
			@(negedge clk_sys); io_din = value; io_strobe = 1'b1;
			@(negedge clk_sys); io_strobe = 1'b0;
		end
	endtask

	task automatic close_command;
		begin
			@(negedge clk_sys); io_uio = 1'b0;
			@(negedge clk_sys);
		end
	endtask

	task automatic vblank;
		begin
			@(negedge clk_sys); hdmi_vbl = 1'b1; output_heartbeat = ~output_heartbeat;
			@(negedge clk_sys); hdmi_vbl = 1'b0;
		end
	endtask

	integer index;
	reg [15:0] words [0:47];
	reg [15:0] crc;
	initial begin
		avalon_payload[0 +: 16] = 16'd1;
		avalon_payload[4*16 +: 16] = 16'h1111;
		output_payload[0 +: 16] = 16'd1;
		repeat(4) @(negedge clk_sys);

		// The independent responder must ignore every existing latch opcode.
		io_uio = 1'b1;
		strobe_word(16'h0057);
		if(response_valid) $fatal(1, "diagnostics responded to latch opcode");
		close_command();
		// Main's intentional pre-ownership one-word disable is retained but harmless.
		io_uio = 1'b1;
		strobe_word(16'h002f);
		strobe_word(16'h0000);
		close_command();
		if(dut.trigger != 0 || dut.legacy_total != 1 || dut.legacy_abort != 1)
			$fatal(1, "pre-ownership partial legacy transaction was not retained safely");

		apply_accepted = 1'b1;
		@(negedge clk_sys); apply_accepted = 1'b0;
		if(expected_route_epoch != (active_route_epoch + 1'd1))
			$fatal(1, "observer route epoch did not match accepted route");
		if(expected_active_seq != pending_seq)
			$fatal(1, "observer sequence did not match accepted pending route");
		vblank();
		vblank();
		vblank();
		if(!monitor_armed) $fatal(1, "monitor did not arm after settling");

		io_uio = 1'b1;
		strobe_word(16'h002f);
		for(index=0; index<10; index=index+1) strobe_word(16'h1000 + index);
		close_command();
		repeat(90) @(negedge clk_sys);
		if(dut.missing_domains != 3'd0)
			$fatal(1, "two-sample mailbox verification did not complete");

		io_uio = 1'b1;
		@(negedge clk_sys); io_din = 16'h005d; io_strobe = 1'b1;
		#1 if(!response_valid || response_data != 16'h4d4d) $fatal(1, "missing control magic");
		@(negedge clk_sys); io_strobe = 1'b0;
		crc = 16'hffff;
		crc = crc_word(crc,16'h005d);
		crc = crc_word(crc,16'd1);
		crc = crc_word(crc,16'd47);
		for(index=0; index<48; index=index+1) begin
			@(negedge clk_sys); io_din = 16'd0; io_strobe = 1'b1;
			#1;
			if(!response_valid) $fatal(1, "response ended at word %0d", index);
			words[index] = response_data;
			if(index < 47) crc = crc_word(crc,response_data);
			@(negedge clk_sys); io_strobe = 1'b0;
		end
		close_command();
		if(words[47] != crc) $fatal(1, "control CRC mismatch expected=%h got=%h", crc, words[47]);
		if(words[0] != 1 || words[2] != 1 || words[10] != 2 ||
		   words[12] != 1 || words[13] != 1 || words[14] != 16'h03ff)
			$fatal(1, "control identity/trigger/mask mismatch");
		for(index=0; index<10; index=index+1)
			if(words[20+index] != 16'h1000 + index) $fatal(1, "legacy payload mismatch");
		$display("video diagnostics control tests passed");
		$finish;
	end

	always @(snapshot_request) begin
		if(snapshot_request) fork
			begin
				wait(dut.avalon_verify_pending && dut.avalon_verify_index == 6'd4);
				@(negedge clk_sys);
				avalon_payload[4*16 +: 16] = 16'h2222;
				@(negedge clk_sys);
				avalon_payload[4*16 +: 16] = 16'h1111;
			end
			begin
				wait(dut.output_verify_pending && dut.output_verify_index == 6'd4);
				@(negedge clk_sys);
				output_payload[4*16 +: 16] = 16'h3333;
				@(negedge clk_sys);
				output_payload[4*16 +: 16] = 16'd0;
			end
		join_none
	end
endmodule

`default_nettype wire
