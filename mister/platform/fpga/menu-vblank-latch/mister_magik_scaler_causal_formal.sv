// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
module mister_magik_scaler_causal_formal;
 reg clk=0;
 always @($global_clock) clk<=!clk;
 (* anyseq *) wire sys, outclk, reset, rd, waitreq, ret, uio, strobe;
 (* anyseq *) wire [7:0] burst;
 (* anyseq *) wire [5:0] physical;
 (* anyseq *) wire [15:0] production, output_state, din;
 mister_magik_scaler_causal_state dut(.clk_100m(clk),.clk_sys(sys),.scaler_clk(outclk),
  .reset_req(reset),.upstream_read(rd),.upstream_wait(waitreq),.upstream_return(ret),
  .upstream_burst(burst),.physical_flags(physical),.production_state(production),
  .output_state(output_state),.io_uio(uio),.io_strobe(strobe),.io_din(din),
  .response_valid(),.response_data());
endmodule
