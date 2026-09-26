// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
// The production-bound scheduler is connected to the unmodified pinned terminator.
// Memory obligations come only from the physical downstream handshake.
module mister_magik_terminator_formal;
  reg clk = 0;
  reg past_valid = 0;
  (* anyseq *) wire reset_n, o_step, waitrequest, returned;
  (* anyseq *) wire vs_edge, schedule_read, copy_retire;
  wire read_request, issued, read_master, upstream_wait, upstream_return;
  wire [7:0] burst_master;
  wire [27:0] address_master;
  wire [8:0] production_words;
  reg reset_0 = 1, reset_1 = 1;
  // sysmem.sv: vbuf_reset_0 <= reset_out; vbuf_reset_1 <= vbuf_reset_0.
  always @(posedge clk) begin
    reset_0 <= !reset_n;
    reset_1 <= reset_0;
  end
  f2sdram_safe_terminator #(.DATA_WIDTH(128), .BURSTCOUNT_WIDTH(8)) terminator (
    .clk(clk), .rst_req_sync(reset_1),
    .waitrequest_master(waitrequest), .burstcount_master(burst_master),
    .address_master(address_master), .readdata_master(128'd0),
    .readdatavalid_master(returned), .read_master(read_master),
    .waitrequest_slave(upstream_wait), .burstcount_slave(8'd128),
    .address_slave(28'h2200000), .readdatavalid_slave(upstream_return),
    .read_slave(read_request), .writedata_slave(128'd0),
    .byteenable_slave(16'hffff), .write_slave(1'b0)
  );
  mister_magik_scaler_completion_formal_dut scheduler (
    .clk(clk), .reset_n(reset_n), .avl_step(1'b1), .o_step(o_step),
    .waitrequest(upstream_wait), .return_valid(upstream_return),
    .vs_edge(vs_edge), .schedule_read(schedule_read),
    .request_copy_retire(copy_retire), .read_request_o(read_request),
    .issue_event_o(issued), .words_remaining_o(production_words)
  );
  reg [10:0] physical_words = 0;
  reg mismatch_seen = 0;
  reg reset_with_read_seen = 0;
  wire accepted = read_master && !waitrequest;
  always @($global_clock) begin
    clk <= !clk;
    past_valid <= 1;
    if (!past_valid) assume(!reset_n);
    if (!clk) begin
      // Ordered nonzero-latency responder; no assumption on outstanding limit.
      if (returned) assume(physical_words != 0);
      case ({accepted, returned})
        2'b10: physical_words <= physical_words + 128;
        2'b01: physical_words <= physical_words - 1;
        2'b11: physical_words <= physical_words + 127;
      endcase
      if (issued != accepted) mismatch_seen <= 1;
      if (!reset_n && read_request) reset_with_read_seen <= 1;
      assert(physical_words < 1920); // reference storage must not wrap silently
`ifdef PROVE_BOUNDARY
      assert(issued == accepted);
      assert(physical_words == production_words);
`endif
      cover(mismatch_seen);
      cover(reset_with_read_seen);
    end
  end
endmodule
