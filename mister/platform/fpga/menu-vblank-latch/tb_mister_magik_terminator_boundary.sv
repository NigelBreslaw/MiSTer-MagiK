// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ns/1ps
module tb_mister_magik_terminator_boundary;
  reg clk=0; always #5 clk=~clk;
  reg reset_n=0, waitrequest=1, schedule_read=0;
  reg reset_0=1, reset_1=1;
  wire read_request, issued, read_master, upstream_wait, terminating;
  wire boundary_busy = !reset_n | reset_0 | reset_1 | terminating;
  reg returned=0, write_slave=0;
  wire write_master;
  integer write_count=0, mode=0,write_burst=4,write_prior=2;
  reg write_owned=0;
  wire [7:0] bus_burst=write_owned ? write_burst[7:0] : 8'd128;
  wire [27:0] bus_address=write_owned ? 28'h2300000 : 28'h2200000;
  always @(posedge clk)if(write_master && !waitrequest)write_count<=write_count+1;
  wire [8:0] production_words;
  wire [1:0] credits;
  wire [6:0] phase;
  wire drain, accepted, pending, request_toggle, ack_sync;
  wire [15:0] production = {2'b0,ack_sync,request_toggle,pending,drain,accepted,credits,phase};
  mister_magik_scaler_causal_state observer (
    .clk_100m(clk),.clk_sys(clk),.scaler_clk(clk),.reset_req(!reset_n),
    .upstream_read(read_request),.upstream_wait(upstream_wait),.upstream_return(returned),
    .upstream_burst(8'd128),.physical_flags({!reset_n,reset_1,read_master,2'b11,returned}),
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
    .clk(clk),.rst_req_sync(reset_1),.busy(terminating),.waitrequest_master(waitrequest),
    .readdata_master(128'd0),.readdatavalid_master(returned),.read_master(read_master),.write_master(write_master),
    .waitrequest_slave(upstream_wait),.burstcount_slave(bus_burst),
    .address_slave(bus_address),.read_slave(read_request),
    .writedata_slave(128'd0),.byteenable_slave(16'hffff),.write_slave(write_slave)
  );
  mister_magik_scaler_completion_formal_dut scheduler (
    .clk(clk),.reset_n(reset_n),.avl_step(1'b1),.o_step(1'b1),
    .boundary_busy(boundary_busy | write_owned),.waitrequest(upstream_wait),.return_valid(returned),.vs_edge(1'b0),
    .schedule_read(schedule_read),.request_copy_retire(1'b0),
    .read_request_o(read_request),.issue_event_o(issued),
    .words_remaining_o(production_words),.return_credits_o(credits),.return_phase_o(phase),
    .return_drain_o(drain),.read_accepted_o(accepted),.completion_pending_o(pending),
    .request_toggle_o(request_toggle),.ack_sync_o(ack_sync)
  );
  task tick; begin @(posedge clk); #1; @(negedge clk); end endtask
  initial begin
    if($value$plusargs("mode=%d",mode))begin end
    if($value$plusargs("write_burst=%d",write_burst))begin end
    if($value$plusargs("write_prior=%d",write_prior))begin end
    $dumpfile("terminator-reset-release.vcd"); $dumpvars(0,tb_mister_magik_terminator_boundary);
    repeat(5) tick(); reset_n=1;
    repeat(8) tick();
    if(mode>=3)begin
      write_owned=1;write_slave=1;waitrequest=0;
      repeat(write_prior)tick();waitrequest=1;
      if(mode==4)begin
        // A final normal write beat must survive a stall before a later reset.
        waitrequest=0;repeat(write_burst-write_prior-1)tick();waitrequest=1;
        repeat(5)tick();waitrequest=0;tick();
        write_slave=0;write_owned=0;
        repeat(3)tick();waitrequest=1;schedule_read=1;
        wait(read_request);@(negedge clk);schedule_read=0;
      end
      if(mode==6)waitrequest=0;
      reset_n=0;write_slave=0;
      repeat(8) tick();reset_n=1;write_owned=0;schedule_read=1;
      if(mode!=4 && mode!=6)repeat(10) begin
        tick();
        if(read_request || !write_master || !boundary_busy)
          $fatal(1,"read launched while terminator owns write");
      end
      if(mode==5)begin
        waitrequest=0;repeat(write_burst-write_prior-1)tick();waitrequest=1;
        repeat(5)begin
          tick();
          if(!write_master || !boundary_busy || read_request)
            $fatal(1,"terminating final write beat lost during stall");
        end
      end
      waitrequest=0;
      wait(read_request);@(negedge clk);schedule_read=0;
      wait(downstream_count==1);@(negedge clk);
      if(upstream_count!=1 || production_words!=128 || write_count!=write_burst)
        $fatal(1,"write handoff failed writes=%0d up=%0d down=%0d",write_count,upstream_count,downstream_count);
      $display("PASS: interrupted write completes before fresh read acceptance");
      $finish;
    end
    schedule_read=1;
    wait(read_request); @(negedge clk); schedule_read=0; reset_n=0;
    // A sustained reset, not a one-cycle formal pulse.
    repeat(8) tick();
    if(mode==1)begin waitrequest=0;tick();end
    reset_n=1;
    tick();
    if(mode!=1)begin
      if (!read_request || !read_master) $fatal(1,"stalled read was withdrawn");
      if(mode==0)repeat(10) begin
        tick();
        if (!read_request || !read_master || !drain)
          $fatal(1,"read or drain lost after boundary release");
      end
      waitrequest=0;tick();
    end
    if (upstream_count != 1 || downstream_count != 1 || production_words != 128)
      $fatal(1,"lost acceptance: up=%0d down=%0d words=%0d",upstream_count,downstream_count,production_words);
    returned=1;
    repeat(128) begin
      if (!drain) $fatal(1,"old response escaped drain");
      tick();
    end
    returned=0;
    repeat(4) tick();
    if (production_words != 0 || drain || upstream_count != 1 || downstream_count != 1)
      $fatal(1,"retained request did not drain exactly once");
    if (observer.first_valid) $fatal(1,"healthy boundary triggered observer");
    $display("PASS: reset release retains one accepted burst and drains all 128 old beats");
    $finish;
  end
  initial begin #5000; $fatal(1,"test timed out"); end
endmodule
