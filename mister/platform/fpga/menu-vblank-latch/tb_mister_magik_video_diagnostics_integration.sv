// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

module tb_mister_magik_video_diagnostics_integration;
	reg clk_sys = 1'b0, clk_100m = 1'b0, hdmi_clk = 1'b0;
	always #5 clk_sys = ~clk_sys;
	always #3.5 clk_100m = ~clk_100m;
	always #4 hdmi_clk = ~hdmi_clk;

	reg hdmi_vbl = 1'b0, hdmi_vs = 1'b0;
	reg io_strobe = 1'b0, io_osd = 1'b0, apply = 1'b0;
	reg [15:0] pending_seq = 16'd23;
	reg [15:0] active_seq = 16'd22, active_route_epoch = 16'd6;
	reg [239:0] frozen_avalon_payload, frozen_output_payload;
	wire snapshot_request, monitor_armed, route_context;
	wire [15:0] generation, expected_route_epoch, expected_active_seq, expected_route_flags;
	wire [31:0] expected_base;
	wire avalon_fault, avalon_ack, output_heartbeat, output_fault, output_ack;
	wire [7:0] avalon_trigger, output_trigger;
	wire [239:0] avalon_payload, output_payload;

	mister_magik_video_diagnostics_control #(
		.HEARTBEAT_TIMEOUT_CYCLES(24'd1000), .SNAPSHOT_TIMEOUT_CYCLES(12'd200)
	) ctrl (
		.clk_sys(clk_sys), .hdmi_vbl(hdmi_vbl), .io_uio(1'b0),
		.io_strobe(io_strobe), .io_osd(io_osd), .io_din(16'd0),
		.apply_accepted(apply), .pending(1'b1), .pending_seq(pending_seq),
		.active_seq(active_seq), .post_count(16'd1),
		.active_route_epoch(active_route_epoch),
		.route_en(1'b1), .route_flt(1'b0), .route_fmt(6'd4),
		.route_width(12'd960), .route_height(12'd540), .route_hmin(12'd0),
		.route_hmax(12'd959), .route_vmin(12'd0), .route_vmax(12'd539),
		.route_base(32'h227e9000), .route_stride(14'd1920),
		.lfb_en(1'b1), .lfb_flt(1'b0), .lfb_fmt(6'd4),
		.lfb_width(12'd960), .lfb_height(12'd540), .lfb_hmin(12'd0),
		.lfb_hmax(12'd959), .lfb_vmin(12'd0), .lfb_vmax(12'd539),
		.lfb_base(32'h227e9000), .lfb_stride(14'd1920),
		.reset_req(1'b0), .reset_out(1'b0), .cfg_done(1'b1),
		.hdmi_pll_locked(1'b1), .output_heartbeat_toggle_async(output_heartbeat),
		.avalon_fault_toggle_async(avalon_fault), .avalon_trigger_async(avalon_trigger),
		.avalon_snapshot_ack_async(avalon_ack),
		.avalon_snapshot_payload_async(avalon_payload),
		.output_fault_toggle_async(output_fault), .output_trigger_async(output_trigger),
		.output_snapshot_ack_async(output_ack),
		.output_snapshot_payload_async(output_payload),
		.snapshot_request_toggle(snapshot_request), .monitor_armed(monitor_armed),
		.diagnostic_generation(generation), .route_context_toggle(route_context),
		.expected_base(expected_base),
		.expected_route_epoch(expected_route_epoch),
		.expected_active_seq(expected_active_seq),
		.expected_route_flags(expected_route_flags), .response_valid(), .response_data()
	);

	mister_magik_video_diagnostics_avalon avalon (
		.clk_100m(clk_100m), .monitor_armed_async(monitor_armed),
		.snapshot_request_toggle_async(snapshot_request),
		.diagnostic_generation_async(generation),
		.route_context_toggle_async(route_context), .expected_base_async(expected_base),
		.expected_route_epoch_async(expected_route_epoch),
		.expected_route_flags_async(expected_route_flags), .frame_marker_async(hdmi_vbl),
		.reset_out_async(1'b0), .vbuf_address(expected_base[31:4]),
		.vbuf_burstcount(8'd128), .vbuf_waitrequest(1'b0),
		.vbuf_readdatavalid(1'b0), .vbuf_read(1'b0), .vbuf_write(1'b0),
		.vbuf_byteenable(16'hffff), .fault_toggle(avalon_fault),
		.fault_trigger(avalon_trigger), .snapshot_ack_toggle(avalon_ack),
		.snapshot_payload(avalon_payload)
	);

	mister_magik_video_diagnostics_output output_observer (
		.hdmi_tx_clk(hdmi_clk), .monitor_armed_async(monitor_armed),
		.snapshot_request_toggle_async(snapshot_request),
		.diagnostic_generation_async(generation),
		.route_context_toggle_async(route_context),
		.expected_route_epoch_async(expected_route_epoch),
		.expected_active_seq_async(expected_active_seq),
		.expected_route_flags_async(expected_route_flags),
		.mux_direct_async(1'b0), .mux_csync_async(1'b0), .reset_req_async(1'b0),
		.cfg_done_async(1'b1), .hdmi_pll_locked_async(1'b1),
		.hdmi_out_d(24'h204080), .hdmi_out_de(1'b1), .hdmi_out_hs(1'b0),
		.hdmi_out_vs(hdmi_vs), .heartbeat_toggle(output_heartbeat),
		.fault_toggle(output_fault), .fault_trigger(output_trigger),
		.snapshot_ack_toggle(output_ack), .snapshot_payload(output_payload)
	);

	task automatic vblank;
		begin
			@(negedge clk_sys); hdmi_vbl = 1'b1;
			@(negedge clk_sys); hdmi_vbl = 1'b0;
		end
	endtask

	initial begin : hdmi_heartbeat
		forever begin
			repeat(12) @(negedge hdmi_clk);
			hdmi_vs = 1'b1;
			@(negedge hdmi_clk); hdmi_vs = 1'b0;
		end
	end

	initial begin
		repeat(5) @(negedge clk_sys);
		apply = 1'b1;
		@(negedge clk_sys); apply = 1'b0; active_seq = pending_seq; active_route_epoch = 16'd7;
		vblank(); vblank(); vblank();
		if(!monitor_armed) $fatal(1, "three-domain monitor did not arm");

		// A fault on the same edge as a later apply must project the accepted context.
		pending_seq = 16'd24;
		@(negedge clk_sys); apply = 1'b1; io_osd = 1'b1; io_strobe = 1'b1;
		@(negedge clk_sys);
		apply = 1'b0; io_osd = 1'b0; io_strobe = 1'b0;
		active_seq = pending_seq; active_route_epoch = 16'd8;
		wait(avalon_ack == snapshot_request && output_ack == snapshot_request);
		frozen_avalon_payload = avalon_payload;
		frozen_output_payload = output_payload;
		repeat(90) begin
			@(negedge clk_sys);
			if(avalon_payload !== frozen_avalon_payload ||
			   output_payload !== frozen_output_payload)
				$fatal(1, "acknowledged native mailbox changed during verification");
		end
		if(ctrl.state != 2'd2 || ctrl.missing_domains != 3'd0)
			$fatal(1, "three-domain capture did not complete coherently");
		if((avalon_payload[16 +: 16] & 16'h000e) != 16'h000e ||
		   (output_payload[16 +: 16] & 16'h000e) != 16'h000e)
			$fatal(1, "cross-domain request did not freeze both observers");
		if(avalon_payload[3*16 +: 16] != generation ||
		   output_payload[3*16 +: 16] != generation)
			$fatal(1, "frozen domain generations do not match control");
		if(output_payload[2*16 +: 16] != MAGIK_VIDEO_DIAGNOSTICS_TRIGGER_NONE)
			$fatal(1, "control-origin snapshot assigned a native output trigger");
		if(avalon_payload[4*16 +: 16] != expected_route_epoch ||
		   output_payload[4*16 +: 16] != expected_route_epoch)
			$fatal(1, "frozen route epochs do not match");
		if(ctrl.frozen_active_route_epoch != expected_route_epoch ||
		   ctrl.frozen_active_seq != expected_active_seq)
			$fatal(1, "control route context does not match native domains");
		$display("video diagnostics three-clock integration tests passed");
		$finish;
	end
endmodule

`default_nettype wire
