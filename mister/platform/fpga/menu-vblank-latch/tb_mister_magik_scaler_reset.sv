// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ps/1ps
module tb_mister_magik_scaler_reset;
 reg clk=0,oclk=0,reset_n=0,rv=0;
 reg avl_enable=1,output_enable=1,output_ce=1,allow_return=1,memory_wait=0;
 wire rd,wr,physical_rd,upwait,up_rv;
 wire [127:0] wdata,updata; wire [15:0] be;
 wire [27:0] addr,physical_addr;wire [7:0] physical_burst,burst;
 wire [15:0] cs,rs,fs;
 reg reset_0=1,reset_1=1;
 integer half_avl=5000,half_output=3367,clock_phase=0;
 integer scenario=0,reset_after=3,reset_hold=1000000,reset_offset=1000;
 integer return_gap=1,partial_beats=64,stop_domain=0,repeat_reset=0;
 integer pending=0,accepted=0,returned=0,cycles=0,old_words=0;
 integer epoch_issued=0,epoch_retired=0,epoch_completed=0,epoch_accepted=0;
 integer resets=0,max_pending=0,reset_pending=0,accepted_at_reset=0;
 integer reset_copy=0,reset_phase=0,post_reset_retired=0;
 reg reset_done=0,was_reset=1;
 reg previous_pre=0;reg [1:0] previous_copy=0;
 reg old_output_reset,old_completion;
 integer before_read,before_copy;
 initial begin
  if($value$plusargs("half_avl=%d",half_avl))begin end
  if($value$plusargs("half_output=%d",half_output))begin end
  if($value$plusargs("clock_phase=%d",clock_phase))begin end
  forever begin #(half_avl);if(avl_enable)clk=~clk;end
 end
 initial begin
  #(clock_phase);
  forever begin #(half_output);if(output_enable)oclk=~oclk;end
 end
 always @(posedge clk)begin reset_0<=!reset_n;reset_1<=reset_0;end
 ascal dut (
  .i_r('0),
  .i_g('0),
  .i_b('0),
  .i_hs('0),
  .i_vs('0),
  .i_fl('0),
  .i_de('0),
  .i_ce(1'b1),
  .i_clk(clk),
  .o_r(),
  .o_g(),
  .o_b(),
  .o_hs(),
  .o_vs(),
  .o_de(),
  .o_vbl(),
  .o_brd(),
  .o_ce(output_ce),
  .o_clk(oclk),
  .o_border('0),
  .o_fb_ena(1'b1),
  .o_fb_hsize(12'd960),
  .o_fb_vsize(12'd540),
  .o_fb_format(6'b000100),
  .o_fb_base(32'h22000000),
  .o_fb_stride(14'd1920),
  .pal1_clk('0),
  .pal1_dw('0),
  .pal1_dr(),
  .pal1_a('0),
  .pal1_wr('0),
  .pal_n('0),
  .pal2_clk('0),
  .pal2_dw('0),
  .pal2_dr(),
  .pal2_a('0),
  .pal2_wr('0),
  .o_lltune(),
  .iauto(1'b1),
  .himin('0),
  .himax('0),
  .vimin('0),
  .vimax('0),
  .i_hdmax(),
  .i_vdmax(),
  .run(1'b1),
  .freeze('0),
  .mode('0),
  .bob_deint('0),
  .htotal(12'd2200),
  .hsstart(12'd2008),
  .hsend(12'd2052),
  .hdisp(12'd1920),
  .hmin(12'd0),
  .hmax(12'd1919),
  .vtotal(12'd1125),
  .vsstart(12'd1084),
  .vsend(12'd1089),
  .vdisp(12'd1080),
  .vmin(12'd0),
  .vmax(12'd1079),
  .vrr('0),
  .vrrmax('0),
  .swblack('0),
  .format(2'b00),
  .poly_clk(clk),
  .poly_dw('0),
  .poly_a('0),
  .poly_wr('0),
  .avl_clk(clk),
  .avl_waitrequest(upwait),
  .avl_readdata(updata),
  .avl_readdatavalid(up_rv),
  .avl_burstcount(burst),
  .avl_writedata(wdata),
  .avl_address(addr),
  .avl_write(wr),
  .avl_read(rd),
  .avl_byteenable(be),
  .magik_fetch_state(fs),
  .magik_return_state(rs),
  .magik_copy_state(cs),
  .reset_na(reset_n));
 f2sdram_safe_terminator #(.DATA_WIDTH(128),
  .BURSTCOUNT_WIDTH(8)) terminator(
 .clk(clk),
  .rst_req_sync(reset_1),
  .waitrequest_master(memory_wait),
 .burstcount_master(physical_burst),
  .address_master(physical_addr),
 .readdata_master(128'b0),
  .readdatavalid_master(rv),
  .read_master(physical_rd),
 .writedata_master(),
  .byteenable_master(),
  .write_master(),
 .waitrequest_slave(upwait),
  .burstcount_slave(burst),
  .address_slave(addr),
 .readdata_slave(updata),
  .readdatavalid_slave(up_rv),
  .read_slave(rd),
 .writedata_slave(wdata),
  .byteenable_slave(be),
  .write_slave(wr));

 // The runner supplies one-time values only for unspecified synthesized storage.
 // Explicit VHDL initial values are preserved; nothing is forced after power-up.
 `include "powerup.vh"

 // Independent responder: obligations originate only at the physical interface.
 always @(negedge clk)begin
  rv <= allow_return && pending>0 && cycles%return_gap==0;
 end
 always @(posedge clk)begin
  cycles=cycles+1;
  if(rv)begin
   if(pending<=0)$fatal(1,"ORPHAN_RETURN");
   pending=pending-1;returned=returned+1;
   if(old_words>0)begin
    if(!dut.avl_return_drain)$fatal(1,"OLD_RETURN_ESCAPED_DRAIN");
    old_words=old_words-1;
   end
  end
  if(physical_rd && !memory_wait)begin
   if(physical_burst!=128)$fatal(1,"BAD_BURST");
   pending=pending+128;accepted=accepted+1;
   if(!reset_n || dut.avl_return_drain)old_words=old_words+128;
   else begin
    epoch_accepted=epoch_accepted+1;
    if(epoch_accepted>epoch_issued)$fatal(1,"UNCHARGED_READ accepted=%0d issued=%0d",epoch_accepted,epoch_issued);
   end
  end
  if(pending>max_pending)max_pending=pending;
  #1;
  if(pending != dut.avl_return_credits*128-dut.avl_return_phase)
   $fatal(1,"PHYSICAL_LEDGER_MISMATCH physical=%0d production=%0d reset=%b",pending,dut.avl_return_credits*128-dut.avl_return_phase,reset_n);
  if(!dut.avl_return_drain && old_words!=0)$fatal(1,"EARLY_DRAIN_RELEASE");
 end

 always @(negedge reset_n)begin
  resets=resets+1;old_words=pending;epoch_accepted=0;
 end
 // Count issue from the real scheduler toggle and retirement from the real FSM.
 // There is no read-credit guard on retirement in this monitor or the DUT.
 always @(posedge oclk or negedge reset_n)begin
  if(!reset_n)begin
   epoch_issued=0;epoch_retired=0;epoch_completed=0;
   previous_pre=0;previous_copy=0;
  end else begin
   old_output_reset=dut.o_reset_na;
   old_completion=dut.o_readdataack;
   before_read=dut.o_readlev;before_copy=dut.o_copylev;
   #1;
   if(old_output_reset)begin
    if(dut.o_read_pre!=previous_pre)epoch_issued=epoch_issued+1;
    if(previous_copy==2 && dut.o_copy==0)begin
     if(before_read==0 || before_copy==0)$fatal(1,"ZERO_CREDIT_RETIREMENT");
     epoch_retired=epoch_retired+1;
     if(reset_done)post_reset_retired=post_reset_retired+1;
    end
    if(old_completion)epoch_completed=epoch_completed+1;
    if(dut.o_readlev!==epoch_issued-epoch_retired)$fatal(1,"READ_CONSERVATION");
    if(dut.o_copylev!==epoch_completed-epoch_retired)$fatal(1,"COPY_CONSERVATION");
    if(dut.o_readlev>2 || dut.o_copylev>dut.o_readlev)$fatal(1,"CREDIT_RANGE");
   end
   previous_pre=dut.o_read_pre;previous_copy=dut.o_copy;
  end
 end

 task reset_core;
 begin
  #(reset_offset);
  reset_pending=pending;accepted_at_reset=accepted;
  reset_copy=dut.o_copy;reset_phase=dut.o_read_pre;
  $display("RESET scenario=%0d pending=%0d phase=%0d copy=%0d accepted=%0d returned=%0d",scenario,pending,reset_phase,reset_copy,accepted,returned);
  reset_n=0;
  if(stop_domain==1)output_enable=0;
  if(stop_domain==2)avl_enable=0;
  #(reset_hold);reset_n=1;
  #(reset_hold);output_enable=1;avl_enable=1;
  if(repeat_reset)begin
   #(reset_hold);reset_n=0;#(reset_hold);reset_n=1;
  end
  reset_done=1;allow_return=1;
 end endtask

 initial begin
  if($value$plusargs("scenario=%d",scenario))begin end
  if($value$plusargs("reset_after=%d",reset_after))begin end
  if($value$plusargs("reset_hold=%d",reset_hold))begin end
  if($value$plusargs("reset_offset=%d",reset_offset))begin end
  if($value$plusargs("return_gap=%d",return_gap))begin end
  if($value$plusargs("partial_beats=%d",partial_beats))begin end
  if($value$plusargs("stop_domain=%d",stop_domain))begin end
  if($value$plusargs("repeat_reset=%d",repeat_reset))begin end
  if($test$plusargs("vcd"))begin
   $dumpfile("reset.vcd");
   $dumpvars(0,clk,oclk,reset_n,reset_0,reset_1,rv,rd,physical_rd,memory_wait,pending,old_words,accepted,returned,cs,rs,fs,epoch_issued,epoch_retired,
    dut.o_read_pre,dut.o_read,dut.avl_read_sync,dut.avl_read_sync2,dut.avl_read_pulse,dut.avl_read_sr,
    dut.avl_readack,dut.o_readack_sync,dut.o_readack_sync2,dut.o_readack,
    dut.avl_return_drain,dut.avl_return_credits,dut.avl_return_phase);
  end
  #100000;reset_n=1;
  wait(accepted==reset_after);wait(cs[3:0]===0);
  case(scenario)
   0: reset_core(); // The incident: idle high or low request phase.
   1: begin // One accepted read, no returned beats.
    allow_return=0;wait(pending==128);reset_core();
   end
   2: begin // Two accepted reads, no returned beats.
    allow_return=0;wait(pending==256);reset_core();
   end
   3: begin // An accepted read partway through its response.
    allow_return=0;wait(pending==128);allow_return=1;
    wait(pending==128-partial_beats);allow_return=0;reset_core();
   end
   4: begin wait(dut.o_read_pre!=dut.o_read);reset_core();end
   5: begin wait(dut.avl_read_sync!=dut.avl_read_sync2);reset_core();end
   6: begin wait(dut.avl_read_pulse);reset_core();end
   7: begin wait(dut.avl_read_sr);reset_core();end
   8: begin wait(dut.avl_readack!=dut.o_readack_sync);reset_core();end
   9: begin wait(dut.o_readack_sync!=dut.o_readack_sync2);reset_core();end
   10: begin wait(dut.o_readack);reset_core();end
   11: begin wait(dut.o_copy==2);reset_core();end
   12: begin // Retained wait-stalled request at the real terminator boundary.
    memory_wait=1;wait(rd);fork
     reset_core();
     begin
      wait(!reset_n);wait(reset_n);@(posedge clk);#1000;memory_wait=0;
     end
    join
   end
   13: begin wait(dut.o_readdataack);reset_core();end
   14: begin wait(dut.o_copy==2);wait(dut.o_copy==0);reset_core();end
   15: begin reset_done=1;end // No-reset control.
   16: begin // A held request accepted while core reset is active.
    memory_wait=1;wait(rd);fork
     reset_core();
     begin #(reset_offset+reset_hold/2);memory_wait=0;end
    join
   end
   17: begin // Return the final old beat just after reset release.
    allow_return=0;wait(pending==128);allow_return=1;
    wait(pending==1);allow_return=0;reset_core();
   end
   default:$fatal(1,"unknown scenario");
  endcase
  #200000000;
  if(epoch_accepted<3 || post_reset_retired<3)$fatal(1,"NO_RECOVERY accepts=%0d retires=%0d",epoch_accepted,post_reset_retired);
  if(old_words!=0)$fatal(1,"OLD_READS_NOT_DRAINED");
  $display("PASS scenario=%0d reset_pending=%0d max_pending=%0d accepts=%0d returns=%0d epoch_issued=%0d epoch_accepted=%0d epoch_retired=%0d state=%h",scenario,reset_pending,max_pending,accepted,returned,epoch_issued,epoch_accepted,epoch_retired,cs);
  $finish;
 end
 initial begin #800000000;$fatal(1,"TIMEOUT scenario=%0d state=%h pending=%0d",scenario,cs,pending);end
endmodule
