// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ns/1ps
module causal_fault_case #(parameter CAUSE=1)(output reg done=0);
 reg clk=0; always #5 clk=~clk;
 reg rd=0,ret=0,physical_read=0,physical_return=0,command_equal=1;
 reg [7:0] burst=128;
 reg [15:0] production=0;
 integer words=0;
 always @(posedge clk) begin
  if(rd) words=words+128;
  if(ret && words>0) words=words-1;
  #1; production=(((words+127)/128)<<7) | ((128-(words%128))%128);
 end
 mister_magik_scaler_causal_state dut(.clk_100m(clk),.clk_sys(clk),.scaler_clk(clk),
  .reset_req(1'b0),.upstream_read(rd),.upstream_wait(1'b0),.upstream_return(ret),
  .upstream_burst(burst),.physical_flags({2'b0,physical_read,command_equal,1'b1,physical_return}),
  .production_state(production),.output_state(16'd0),.io_uio(1'b0),.io_strobe(1'b0),.io_din(16'd0),
  .response_valid(),.response_data());
 task tick; begin @(posedge clk); #2; @(negedge clk); end endtask
 initial begin
  repeat(3) tick();
  case(CAUSE)
   1: rd=1;
   2: physical_read=1;
   3: begin rd=1; physical_read=1; command_equal=0; end
   4: production=1;
   5,8: begin rd=1;physical_read=1;repeat(2)tick(); if(CAUSE==8)tick(); end
   6: physical_return=1;
   7: begin rd=1;physical_read=1;burst=64;end
   9: ret=1;
  endcase
  #1;
  if(dut.cause!=CAUSE) $fatal(1,"fault %0d classified as %0d",CAUSE,dut.cause);
  tick(); rd=0;ret=0;physical_read=0;physical_return=0;command_equal=1;burst=128;
  if(!dut.first_valid || dut.first_source[11:8]!=(CAUSE==8 ? 5:CAUSE))
   $fatal(1,"first-event capture missing for cause %0d",CAUSE);
  if((CAUSE==6 || CAUSE==7 || CAUSE==8) && dut.ledger_valid)
   $fatal(1,"invalid physical ledger was not marked invalid");
  done=1;
 end
endmodule
module tb_mister_magik_causal_faults;
 wire [8:0] done;
 genvar n;
 generate for(n=1;n<=9;n=n+1)begin: fault
  causal_fault_case #(.CAUSE(n)) test(done[n-1]);
 end endgenerate
 initial begin wait(&done);$display("PASS: all nine causal predicates and invalid-ledger cases");$finish;end
 initial begin #5000;$fatal(1,"fault matrix timeout");end
endmodule
