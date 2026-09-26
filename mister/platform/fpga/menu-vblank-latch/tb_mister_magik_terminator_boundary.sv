// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ns/1ps
module tb_mister_magik_terminator_boundary;
  reg clk=0; always #5 clk=~clk;
  reg reset_n=0, waitrequest=1, schedule_read=0;
  reg reset_0=1, reset_1=1;
  wire read_request, issued, read_master, upstream_wait;
  wire [8:0] production_words;
  wire [1:0] credits;
  wire [6:0] phase;
  wire drain, accepted, pending, request_toggle, ack_sync;
  wire [15:0] production = {2'b0,ack_sync,request_toggle,pending,drain,accepted,credits,phase};
  mister_magik_scaler_causal_state observer (
    .clk_100m(clk),.clk_sys(clk),.scaler_clk(clk),.reset_req(!reset_n),
    .upstream_read(read_request),.upstream_wait(upstream_wait),.upstream_return(1'b0),
    .upstream_burst(8'd128),.physical_flags({!reset_n,reset_1,read_master,3'b110}),
    .production_state(production),.output_state(16'b0),.io_uio(1'b0),.io_strobe(1'b0),.io_din(16'b0),
    .response_valid(),.response_data()
  );
  integer upstream_count=0, downstream_count=0;
  always @(posedge clk) begin
    reset_0 <= !reset_n;
    reset_1 <= reset_0;
    if (issued) upstream_count <= upstream_count+1;
    if (read_master && !waitrequest) downstream_count <= downstream_count+1;
  end
  f2sdram_safe_terminator #(.DATA_WIDTH(128),.BURSTCOUNT_WIDTH(8)) terminator (
    .clk(clk),.rst_req_sync(reset_1),.waitrequest_master(waitrequest),
    .readdata_master(128'd0),.readdatavalid_master(1'b0),.read_master(read_master),
    .waitrequest_slave(upstream_wait),.burstcount_slave(8'd128),
    .address_slave(28'h2200000),.read_slave(read_request),
    .writedata_slave(128'd0),.byteenable_slave(16'hffff),.write_slave(1'b0)
  );
  mister_magik_scaler_completion_formal_dut scheduler (
    .clk(clk),.reset_n(reset_n),.avl_step(1'b1),.o_step(1'b1),
    .waitrequest(upstream_wait),.return_valid(1'b0),.vs_edge(1'b0),
    .schedule_read(schedule_read),.request_copy_retire(1'b0),
    .read_request_o(read_request),.issue_event_o(issued),
    .words_remaining_o(production_words),.return_credits_o(credits),.return_phase_o(phase),
    .return_drain_o(drain),.read_accepted_o(accepted),.completion_pending_o(pending),
    .request_toggle_o(request_toggle),.ack_sync_o(ack_sync)
  );
  task tick; begin @(posedge clk); #1; @(negedge clk); end endtask
  initial begin
    $dumpfile("terminator-reset-release.vcd"); $dumpvars(0,tb_mister_magik_terminator_boundary);
    repeat(5) tick(); reset_n=1;
    repeat(8) tick(); schedule_read=1;
    wait(read_request); @(negedge clk); schedule_read=0; reset_n=0;
    // A sustained reset, not a one-cycle formal pulse.
    repeat(8) tick(); reset_n=1;
    tick();
    if (read_request) $fatal(1,"source request should be gated after release");
    if (!read_master) $fatal(1,"terminator should retain stalled request");
    waitrequest=0;
    tick();
    if (upstream_count != 0 || downstream_count != 1 || production_words != 0)
      $fatal(1,"expected downstream-only acceptance: up=%0d down=%0d words=%0d",upstream_count,downstream_count,production_words);
    if (!observer.first_valid || observer.first_source[11:8] != 2 || observer.depth != 1)
      $fatal(1,"observer missed downstream-only acceptance cause=%0d depth=%0d",observer.first_source[11:8],observer.depth);
    $display("PASS: sustained reset release exposes downstream-only accepted burst (up=0 down=1 production_words=0)");
    $finish;
  end
  initial begin #5000; $fatal(1,"test timed out"); end
endmodule
