// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
`timescale 1ns/1ps
`default_nettype none
// Passive evidence only. No output from this module feeds functional logic.
// 0x68 selects the immutable first source-domain event; 0x69 selects live state.
// 0x6a reads the completed snapshot (schema, flags, phases, output, CRC).
// Snapshot commands never clear event evidence or memory obligations.
module mister_magik_scaler_causal_state (
 input wire clk_100m, clk_sys, scaler_clk,
 input wire reset_req, upstream_read, upstream_wait, upstream_return,
 input wire [7:0] upstream_burst,
 input wire [5:0] physical_flags,
 input wire [15:0] production_state, output_state,
 input wire io_uio, io_strobe,
 input wire [15:0] io_din,
 output wire response_valid,
 output reg [15:0] response_data
);
 // physical_flags: {reset_out, terminator_reset, read, command_equal, burst_ok, return}.
 wire down_accept = physical_flags[3] && !upstream_wait;
 wire up_accept = upstream_read && !upstream_wait;
 wire returned = physical_flags[0];
 // Radix-128 physical ledger: 0..3 bursts, 0..127 returned beats.
 // The fourth unretired burst invalidates the ledger instead of wrapping it.
 reg [1:0] depth = 0;
 reg [6:0] phase = 0;
 reg ledger_valid = 1;
 wire last_return = returned && depth != 0 && phase == 127;
 wire orphan = returned && depth == 0;
 wire bad_shape = (down_accept && !physical_flags[1]) || (up_accept && upstream_burst != 128);
 wire overflow = down_accept && depth == 3 && !last_return;
 reg [3:0] cause;
 always @* begin
  cause = 0;
  if (bad_shape) cause = 7;
  else if (ledger_valid && orphan) cause = 6;
  else if (ledger_valid && overflow) cause = 8;
  else if (up_accept && !down_accept) cause = 1;
  else if (down_accept && !up_accept) cause = 2;
  else if (down_accept && !physical_flags[2]) cause = 3;
  else if (ledger_valid && (production_state[8:7] != depth || production_state[6:0] != phase)) cause = 4;
  else if (ledger_valid && down_accept && depth >= 2 && !last_return) cause = 5;
  else if (upstream_return != returned) cause = 9;
 end
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *) reg reset_meta = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *) reg reset_sync = 0;
 always @(posedge clk_100m) begin reset_meta <= reset_req; reset_sync <= reset_meta; end
 wire [15:0] live_flags = {1'b1,2'b00,ledger_valid,cause,
   physical_flags[4],reset_sync,(production_state[12]^production_state[13]),
   production_state[11:7]};
 wire [31:0] live_source = {production_state[6:0],depth,phase,live_flags};
 reg [31:0] first_source = 0;
 reg first_valid = 0;
 always @(posedge clk_100m) begin
  if (ledger_valid) begin
   if (bad_shape || orphan || overflow) ledger_valid <= 0;
   else begin
    case ({down_accept,last_return})
     2'b10: depth <= depth+1'b1;
     2'b01: depth <= depth-1'b1;
     default: depth <= depth;
    endcase
    if (returned && depth != 0) phase <= phase+1'b1;
   end
  end
  if (!first_valid && cause != 0) begin
   first_source <= live_source;
   first_valid <= 1;
  end
 end

 // Closed-loop source mailbox. Selector and payload remain held until response.
 (* preserve, dont_replicate *) reg capture_request = 0;
 (* preserve, dont_replicate *) reg select_first = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *) reg capture_meta = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *) reg capture_sync = 0;
 (* preserve, dont_replicate *) reg response_toggle = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *) reg response_meta = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *) reg response_sync = 0;
 reg capture_busy = 0;
 reg [5:0] output_wait = 0;
 (* preserve, dont_replicate *) reg output_request = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *) reg output_request_meta = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *) reg output_request_sync = 0;
 (* preserve, dont_replicate *) reg output_response = 0;
 (* preserve, dont_replicate *) reg [15:0] output_hold = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *) reg output_response_meta = 0;
 (* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *) reg output_response_sync = 0;
 always @(posedge scaler_clk) begin
  output_request_meta <= output_request;
  output_request_sync <= output_request_meta;
  if (output_request_sync != output_response) begin
   output_hold <= output_state;
   output_response <= output_request_sync;
  end
 end
 (* preserve, dont_replicate *) reg [31:0] snapshot = 0;
 reg [15:0] crc_work = 0;
 reg [6:0] crc_phase = 0;
 reg crc_busy = 0;
 reg output_requested = 0;
 function automatic [15:0] crc_bit(input [15:0] old, input data_bit);
  crc_bit = {old[14:0],1'b0} ^ ((old[15] ^ data_bit) ? 16'h1021 : 16'd0);
 endfunction
 function automatic [15:0] crc_word(input [15:0] old, input [15:0] word_value);
  integer bit_index; reg [15:0] value;
  begin
   value=old;
   for(bit_index=15;bit_index>=0;bit_index=bit_index-1) value=crc_bit(value,word_value[bit_index]);
   crc_word=value;
  end
 endfunction
 localparam [15:0] CRC_SEED = crc_word(crc_word(crc_word(crc_word(16'hffff,16'h006a),16'd24),16'd4),16'd24);
 always @(posedge clk_100m) begin
  capture_meta <= capture_request;
  capture_sync <= capture_meta;
  output_response_meta <= output_response;
  output_response_sync <= output_response_meta;
  if (!capture_busy && capture_sync != response_toggle) begin
   snapshot[31:0] <= select_first ? (first_valid ? first_source : 32'd0) : live_source;
   snapshot[13] <= select_first;
   snapshot[14] <= 0;
   capture_busy <= 1;
   output_wait <= 0;
   // Never reuse an unacknowledged output-domain mailbox after a timeout.
   output_requested <= output_request == output_response_sync;
   if (output_request == output_response_sync) output_request <= !output_request;
  end else if (capture_busy && !crc_busy) begin
   if ((output_requested && output_response_sync == output_request) || &output_wait) begin
    if (output_requested && output_response_sync == output_request) begin
     snapshot[14] <= 1;
    end
    crc_work <= CRC_SEED;
    crc_phase <= 0;
    crc_busy <= 1;
   end else output_wait <= output_wait + 1'b1;
  end else if (crc_busy) begin
   // Wire permutation serializes low-word-first, MSB first within each word.
   crc_work <= crc_bit(crc_work,crc_phase[5] ? (snapshot[14] && output_hold[~crc_phase[3:0]]) : snapshot[15]);
   // Rotate the source bank through exactly 32 CRC steps. It returns to its
   // original value before publication, avoiding a wide indexed source mux.
   if (!crc_phase[5]) snapshot <= {snapshot[30:16],snapshot[15],snapshot[14:0],snapshot[31]};
   if (crc_phase == 47) begin
    crc_busy <= 0;
    capture_busy <= 0;
    response_toggle <= capture_sync;
   end else crc_phase <= crc_phase + 1'b1;
  end
 end
 reg has_command = 0;
 reg selected_read = 0;
 reg [2:0] word_count = 0;
 reg request_issued = 0;
 wire command_start = io_uio && io_strobe && !has_command;
 wire command_data = io_uio && io_strobe && has_command;
 wire ready = request_issued && response_sync == capture_request;
 wire start_capture = command_start && (io_din[7:0]==8'h68 || io_din[7:0]==8'h69) && (!request_issued || ready);
 wire start_read = command_start && io_din[7:0]==8'h6a && ready;
 assign response_valid = start_capture || start_read || (command_data && selected_read && word_count<5);
 always @* begin
  response_data=0;
  if (start_capture) response_data=io_din[7:0]==8'h68 ? 16'h4d58 : 16'h4d59;
  else if (start_read) response_data=16'h4d5a;
  else if (command_data && selected_read && word_count<5) begin
   case(word_count)
    0: response_data=24;
    1: response_data=snapshot[15:0];
    2: response_data=snapshot[31:16];
    3: response_data=snapshot[14] ? output_hold : 16'd0;
    4: response_data=crc_work;
    default: response_data=0;
   endcase
  end
 end
 always @(posedge clk_sys) begin
  response_meta <= response_toggle;
  response_sync <= response_meta;
  if (start_capture) begin
   select_first <= io_din[7:0]==8'h68;
   capture_request <= !capture_request;
   request_issued <= 1;
  end
  if (command_start) begin
   has_command <= 1;
   selected_read <= start_read;
   word_count <= 0;
  end else if (command_data && selected_read && word_count<5) word_count <= word_count+1'b1;
  if (!io_uio) begin
   has_command <= 0;
   selected_read <= 0;
  end
 end
`ifdef FORMAL
 reg f_past=0;
 (* keep *) wire cover_phase45 = first_valid && first_source[11:8]==5 && first_source[22:16]==45;
 (* keep *) wire cover_drain = first_valid && depth==0 && ledger_valid;
 (* keep *) wire cover_invalid = !ledger_valid;
 (* keep *) wire cover_timeout = crc_busy && crc_phase==47 && !snapshot[14];
 always @(posedge clk_100m) begin
  f_past<=1;
  assert(depth != 0 || phase == 0);
  assert(!crc_busy || capture_busy);
  assert(!crc_busy || crc_phase <= 47);
  if(f_past) begin
   if($past(first_valid)) begin
    assert(first_valid);
    assert(first_source==$past(first_source));
   end
   if(!$past(ledger_valid)) begin
    assert(!ledger_valid);
    assert(depth==$past(depth) && phase==$past(phase));
   end
   if($past(ledger_valid && !bad_shape && !orphan && !overflow)) begin
    assert(({depth,7'd0}-{2'd0,phase}) ==
     ($past({depth,7'd0}-{2'd0,phase}) + ($past(down_accept)?9'd128:9'd0) - ($past(returned)?9'd1:9'd0)));
   end
   if($past(crc_busy)) begin
    assert(crc_work==crc_bit($past(crc_work),$past(crc_phase[5]) ?
      ($past(snapshot[14]) && $past(output_hold[~crc_phase[3:0]])) : $past(snapshot[15])));
    if(!$past(crc_phase[5]))
     assert(snapshot=={$past(snapshot[30:16]),$past(snapshot[15]),$past(snapshot[14:0]),$past(snapshot[31])});
    else assert(snapshot==$past(snapshot));
   end
   if(response_toggle != $past(response_toggle))
    assert($past(crc_busy && crc_phase==47));
  end
  cover(first_valid && first_source[11:8]==5 && first_source[22:16]==45);
  cover(first_valid && depth==0 && ledger_valid);
  cover(!ledger_valid);
  cover(crc_busy && crc_phase==47 && !snapshot[14]);
 end
`endif
endmodule
`default_nettype wire
