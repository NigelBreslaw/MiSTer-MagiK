// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ns/1ps
module tb_mister_magik_scaler_causal_state;
 reg clk=0, sys=0, outclk=0, out_run=1;
 always #5 clk=~clk;
 always #7 sys=~sys;
 always #9 if(out_run) outclk=~outclk;
 reg reset=0, rd=0, ret=0, waitreq=0;
 reg [5:0] physical=6'b000110;
 reg [15:0] production=0, output_state=16'h531a;
 reg io_uio=0, io_strobe=0;
 reg [15:0] io_din=0;
 wire valid; wire [15:0] response;
 integer words=0;
 always @(posedge clk) begin
  if(rd && !waitreq) words=words+128;
  if(ret) words=words-1;
  // Independent test source accounting, sampled after the active edge.
  #1; production=(((words+127)/128)<<7) | ((128-(words%128))%128);
 end
 mister_magik_scaler_causal_state dut(.clk_100m(clk),.clk_sys(sys),.scaler_clk(outclk),
  .reset_req(reset),.upstream_read(rd),.upstream_wait(waitreq),.upstream_return(ret),
  .upstream_burst(8'd128),.physical_flags(physical),.production_state(production),
  .output_state(output_state),.io_uio(io_uio),.io_strobe(io_strobe),.io_din(io_din),
  .response_valid(valid),.response_data(response));
 task tick; begin @(posedge clk); #2; @(negedge clk); end endtask
 task spi(input [15:0] word_value, output [15:0] received);
  begin @(negedge sys);io_din=word_value;io_strobe=1;#1;received=valid?response:0;
   @(posedge sys);#1;@(negedge sys);io_strobe=0;end
 endtask
 task end_spi; begin @(negedge sys);io_uio=0;repeat(2) @(posedge sys);end endtask
 reg [15:0] ack;
 reg [15:0] record [0:4];
 reg [15:0] saved [0:2];
 integer i;
 task capture(input [15:0] command);
  begin
   io_uio=1;spi(command,ack);
   if(ack != (command==16'h68 ? 16'h4d58 : 16'h4d59)) $fatal(1,"select ack %h",ack);
   end_spi();
   repeat(400) tick();
   io_uio=1;spi(16'h6a,ack);if(ack!=16'h4d5a) $fatal(1,"read not ready");
   for(i=0;i<5;i=i+1) spi(0,record[i]);
   spi(0,ack);if(ack!=0) $fatal(1,"overlong read");
   end_spi();
   $display("RECORD %h %h %h %h %h",record[0],record[1],record[2],record[3],record[4]);
  end
 endtask
 initial begin
  repeat(5) tick();
  // Two accepted bursts then 45 returned beats reproduces the incident boundary.
  rd=1;physical[3]=1;repeat(2)tick();rd=0;physical[3]=0;
  ret=1;physical[0]=1;repeat(45)tick();ret=0;physical[0]=0;
  rd=1;physical[3]=1;tick();rd=0;physical[3]=0;
  capture(16'h68);
  if(record[0]!=24 || record[1][11:8]!=5 || record[2][8:7]!=2 || record[2][6:0]!=45 || record[2][15:9]!=45)
   $fatal(1,"wrong excess-read context");
  for(i=0;i<3;i=i+1) saved[i]=record[i];
  // Continuing physical progress must remain observable after the first event.
  ret=1;physical[0]=1;repeat(60)tick();ret=0;physical[0]=0;
  capture(16'h69);
  if(record[2][8:7]!=3 || record[2][6:0]!=105 || record[1][13]) $fatal(1,"live ledger stopped after first event");
  capture(16'h68);
  for(i=0;i<3;i=i+1) if(record[i]!=saved[i]) $fatal(1,"first record overwritten");
  // A stopped output clock must not prevent retrieval of source evidence.
  out_run=0;capture(16'h68);
  if(record[1][14] || record[3]!=0 || record[1][11:8]!=5) $fatal(1,"stopped output validity");
  out_run=1;repeat(20)tick();capture(16'h69);
  if(!record[1][14]) $fatal(1,"output mailbox did not recover");
  $display("PASS: causal first/live records, continuing ledger, bounded stopped-clock capture");$finish;
 end
 initial begin #1000000;$fatal(1,"timeout");end
endmodule
