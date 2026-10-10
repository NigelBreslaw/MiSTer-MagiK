// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
// The production-bound scheduler is connected to the patched production terminator.
// Memory obligations come only from the physical downstream handshake.
module mister_magik_terminator_formal;
  reg clk = 0;
  reg past_valid = 0;
  (* anyseq *) wire reset_n, o_step, waitrequest, returned, start_write, external_reset;
  (* anyseq *) wire vs_edge, schedule_read, copy_retire;
  wire read_request, issued, read_master, upstream_wait, upstream_return, terminating;
  wire [7:0] burst_master;
  wire [27:0] address_master;
  wire [8:0] production_words;
  reg reset_0 = 1, reset_1 = 1;
  wire reset_out = !reset_n | external_reset;
  wire boundary_busy = reset_out | reset_0 | reset_1 | terminating;
  wire read_active, drain, write_master;
  reg [2:0] write_left = 0;
  wire launch_write = start_write && reset_n && !boundary_busy && !drain &&
                      !read_active && write_left == 0;
  wire write_slave = write_left != 0 && reset_n;
  // Abstract legal scaler write bursts. They exclude new read launches and
  // disappear at core reset; the real terminator owns their remaining beats.
  always @(posedge clk) begin
    if (!reset_n) write_left <= 0;
    else if (launch_write) write_left <= 4;
    else if (write_slave && !waitrequest) write_left <= write_left - 1'b1;
  end
  // sysmem.sv: vbuf_reset_0 <= reset_out; vbuf_reset_1 <= vbuf_reset_0.
  always @(posedge clk) begin
    reset_0 <= reset_out;
    reset_1 <= reset_0;
  end
  f2sdram_safe_terminator #(.DATA_WIDTH(128), .BURSTCOUNT_WIDTH(8)) terminator (
    .clk(clk), .rst_req_sync(reset_1), .busy(terminating),
    .waitrequest_master(waitrequest), .burstcount_master(burst_master),
    .address_master(address_master), .readdata_master(128'd0),
    .readdatavalid_master(returned), .read_master(read_master), .write_master(write_master),
    .waitrequest_slave(upstream_wait), .burstcount_slave(write_slave ? 8'd4 : 8'd128),
    .address_slave(write_slave ? 28'h2300000 : 28'h2200000), .readdatavalid_slave(upstream_return),
    .read_slave(read_request), .writedata_slave(128'd0),
    .byteenable_slave(16'hffff), .write_slave(write_slave)
  );
  mister_magik_scaler_completion_formal_dut scheduler (
    .clk(clk), .reset_n(reset_n), .avl_step(1'b1), .o_step(o_step),
    .boundary_busy(boundary_busy | (write_left != 0) | launch_write), .waitrequest(upstream_wait), .return_valid(upstream_return),
    .vs_edge(vs_edge), .schedule_read(schedule_read),
    .request_copy_retire(copy_retire), .read_request_o(read_request),
    .read_active_o(read_active), .return_drain_o(drain),
    .issue_event_o(issued), .words_remaining_o(production_words)
  );
  reg [10:0] physical_words = 0;
  reg [2:0] write_words = 0;
  (* keep *) reg write_release_seen = 0;
  (* keep *) reg read_after_write_seen = 0;
  reg reset_with_read_seen = 0;
  wire accepted = read_master && !waitrequest;
  always @($global_clock) begin
    clk <= !clk;
    past_valid <= 1;
    if (!past_valid) assume(!reset_n);
    if (!clk) begin
      if (write_master && !waitrequest)
        write_words <= write_words == 0 ? 3 : write_words - 1'b1;
      if (accepted) assert(write_words == 0);
      // Ordered nonzero-latency responder; no assumption on outstanding limit.
      if (returned) assume(physical_words != 0);
      case ({accepted, returned})
        2'b10: physical_words <= physical_words + 128;
        2'b01: physical_words <= physical_words - 1;
        2'b11: physical_words <= physical_words + 127;
      endcase
      if (reset_n && terminating && write_master) write_release_seen <= 1;
      if (write_release_seen && accepted) read_after_write_seen <= 1;
      if (!reset_n && read_request) reset_with_read_seen <= 1;
      assert(physical_words < 1920); // reference storage must not wrap silently
`ifdef PROVE_BOUNDARY
      assert(issued == accepted);
      assert(physical_words == production_words);
`endif
      assert(!(read_master && write_master));
      if (accepted) begin
        assert(burst_master == 128);
        assert(address_master == 28'h2200000);
      end
      cover(read_after_write_seen);
      cover(reset_with_read_seen);
    end
  end
endmodule
