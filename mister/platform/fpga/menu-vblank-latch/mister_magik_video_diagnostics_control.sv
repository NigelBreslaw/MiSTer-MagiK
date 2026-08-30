// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Source-clock evidence collector and closed-loop multi-cycle-path snapshot.
// The live bus is consumed only on scaler_clk. Every source-clock event in one
// complete scheduler revolution is ORed directly into evidence_hold before
// response_toggle crosses to the destination. evidence_hold then remains
// immutable until another request, so the destination never samples a changing
// asynchronous multi-bit value. Failure to complete a revolution is detected
// by the existing destination-domain watchdog and fails closed.
module mister_magik_scaler_scheduler_snapshot (
	input  wire        scaler_clk,
	input  wire [15:0] live_state,
	input  wire        request_toggle,
	output reg         response_toggle = 1'b0,
	output reg  [15:0] evidence_hold = 16'h0001
`ifdef FORMAL
	,output wire        formal_capture_active
	,output wire [15:0] formal_accumulated_evidence
	,output wire [15:0] formal_event_evidence
`endif
);
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg request_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg request_sync = 1'b0;
	reg capture_active = 1'b0;
	reg [1:0] previous_output_state = 2'd0;
	reg previous_vertical_pixel_enable = 1'b0;
	reg previous_vertical_carry = 1'b0;

	wire [1:0] output_state = live_state[1:0];
	wire hsync_entry = previous_output_state != 2'd1 && output_state == 2'd1;
	wire left_hsync = previous_output_state == 2'd1 && output_state != 2'd1;
	wire [15:0] event_evidence = {
		live_state[12],
		(output_state == 2'd3),
		(previous_output_state == 2'd2 && output_state == 2'd3),
		(output_state == 2'd2 && live_state[8]),
		(output_state == 2'd2),
		(left_hsync && output_state == 2'd0 && !previous_vertical_carry),
		(left_hsync && output_state == 2'd0 && !previous_vertical_pixel_enable),
		(left_hsync && output_state == 2'd0),
		(left_hsync && output_state == 2'd2),
		left_hsync,
		(previous_output_state == 2'd1 && output_state == 2'd1),
		(output_state == 2'd1),
		(output_state == 2'd0 && live_state[4]),
		live_state[3],
		live_state[2],
		1'b0
	};
	wire [15:0] next_evidence = evidence_hold | event_evidence;

`ifdef FORMAL
	assign formal_capture_active = capture_active;
	assign formal_accumulated_evidence = evidence_hold;
	assign formal_event_evidence = event_evidence;
`endif

	always @(posedge scaler_clk) begin
		request_meta <= request_toggle;
		request_sync <= request_meta;
		previous_output_state <= output_state;
		previous_vertical_pixel_enable <= live_state[5];
		previous_vertical_carry <= live_state[6];

		if(request_sync == response_toggle) begin
			capture_active <= 1'b0;
		end
		else if(!capture_active) begin
			// Bit zero is a constant completed-window marker and therefore is not
			// a physical CDC payload register. Accumulate only bits 15:1.
			capture_active <= 1'b1;
			evidence_hold[15:1] <= event_evidence[15:1];
		end
		else if(hsync_entry && evidence_hold[4]) begin
			evidence_hold[15:1] <= next_evidence[15:1];
			response_toggle <= request_sync;
			capture_active <= 1'b0;
		end
		else begin
			evidence_hold[15:1] <= next_evidence[15:1];
		end
	end
endmodule

// Replacement-only passive observer at the external scaler Avalon boundary.
// reset_req is observed as data: it never clears accepted return obligations,
// telemetry, or the publication handshake. A no-request timeout freezes the
// packed output-scheduler gates supplied by ascal. No observer output drives
// production.
module mister_magik_scaler_fetch_liveness_state #(
	parameter [23:0] WATCHDOG_LIMIT = 24'hffffff,
	parameter [2:0] RESET_QUALIFY_LIMIT = 3'd4
) (
	input  wire        clk_100m,
	input  wire        clk_sys,
	input  wire        scaler_clk,
	input  wire        reset_req,
	input  wire [27:0] vbuf_address,
	input  wire [7:0]  vbuf_burstcount,
	input  wire        vbuf_waitrequest,
	input  wire        vbuf_readdatavalid,
	input  wire        vbuf_read,
	input  wire [15:0] scaler_diag_state,
	input  wire        io_uio,
	input  wire        io_strobe,
	input  wire [15:0] io_din,
	output wire        response_valid,
	output reg  [15:0] response_data
`ifdef FORMAL
	,output wire [1:0] formal_fifo_count
	,output wire [6:0] formal_return_phase
	,output wire formal_first_stall_valid
	,output wire formal_observer_fault
	,output wire [15:0] formal_frozen_state
	,output wire formal_publication_generation
	,output wire formal_acknowledge_sync
	,output wire [47:0] formal_published_bundle
	,output wire [3:0] formal_publication_sequence
	,output wire formal_publish_crc_busy
	,output wire [1:0] formal_publish_crc_word
	,output wire formal_enqueue
	,output wire formal_dequeue
	,output wire formal_return_has_entry
	,output wire formal_expected_progress
	,output wire formal_watchdog_terminal
	,output wire formal_observer_fault_event
	,output wire formal_request_cancel_event
`endif
);

`include "mister_magik_video_diagnostics_protocol.svh"

	localparam [7:0] REQUIRED_BURSTCOUNT =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_REQUIRED_BURSTCOUNT;
	localparam [23:0] CONTRACT_WATCHDOG_LIMIT =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_WATCHDOG_CYCLES;
	localparam [2:0] CONTRACT_RESET_QUALIFY_LIMIT =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_RESET_QUALIFY_CYCLES;
	localparam [1:0] CONTRACT_SNAPSHOT_HSYNC_ENTRIES =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_SNAPSHOT_HSYNC_ENTRIES;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg reset_meta = 1'b1;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg reset_sync = 1'b1;
	reg reset_sync_d = 1'b1;
	reg [2:0] reset_low_count = 3'd0;
	wire reset_qualified = reset_low_count >= RESET_QUALIFY_LIMIT;
	reg ever_qualified = 1'b0;

	// Independent two-entry accepted-obligation scoreboard. Production caps the
	// external scaler reads at two; obligations remain live across reset_req.
	reg fifo_wrap0 = 1'b0;
	reg fifo_wrap1 = 1'b0;
	reg [1:0] fifo_count = 2'd0;
	reg [6:0] return_phase = 7'd0;
	reg [15:0] previous_address = 16'd0;
	reg previous_address_valid = 1'b0;

	reg normal_liveness_seen = 1'b0;
	reg address_wrap_seen = 1'b0;
	reg blocked_request_seen = 1'b0;
	reg [23:0] progress_watchdog = 24'd0;
	reg snapshot_request_toggle = 1'b0;
	reg snapshot_pending = 1'b0;
	reg snapshot_invalidated = 1'b0;
	wire snapshot_response_toggle;
	wire [15:0] snapshot_evidence_hold;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg snapshot_response_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg snapshot_response_sync = 1'b0;

	// Sticky first-stall/fault evidence. Rolling live state and publication keep
	// advancing after this bank freezes.
	reg reset_ambiguity = 1'b0;
	reg reset_since_normal_liveness = 1'b0;
	reg no_request_seen = 1'b0;
	reg [2:0] frozen_cause =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_NONE;
	wire observer_fault = !no_request_seen &&
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_OBSERVER_FAULT;
	wire first_stall_valid = no_request_seen ||
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_ACCEPT_BLOCKED ||
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_FIRST_RETURN_MISSING ||
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE ||
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_REQUEST_CANCELLED;
	wire accept_blocked_seen = !no_request_seen &&
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_ACCEPT_BLOCKED;
	wire first_return_missing = !no_request_seen &&
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_FIRST_RETURN_MISSING;
	wire return_incomplete = !no_request_seen &&
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE;
	wire request_cancelled = !no_request_seen &&
		frozen_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_REQUEST_CANCELLED;
	reg [6:0] frozen_return_phase = 7'd0;
	reg [1:0] frozen_fifo_depth = 2'd0;
	reg [3:0] frozen_address_fold = 4'd0;
	// Dedicated destination bank for the closed-loop scaler snapshot. Keeping
	// it separate from fault attribution prevents synthesis from proving away
	// individual CDC payload paths through mutually exclusive fault branches.
	(* preserve, dont_replicate *) reg [15:0] scheduler_snapshot_state = 16'd0;
	// The destination acknowledges only after a complete command. The source
	// publication bank remains immutable until that acknowledgement returns.
	reg [3:0] publication_sequence = 4'd0;
	reg [15:0] published_flags = 16'd0;
	reg [15:0] published_state = 16'd0;
	(* preserve, dont_replicate *) reg publication_generation = 1'b0;
	reg publish_crc_busy = 1'b0;
	reg [1:0] publish_crc_word = 2'd0;
	reg [15:0] publish_crc_work = 16'd0;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg acknowledge_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg acknowledge_sync = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg generation_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg generation_sync = 1'b0;
	reg acknowledged_generation = 1'b0;

	reg has_command = 1'b0;
	reg command_selected = 1'b0;
	reg [2:0] word_count = 3'd0;
	reg [15:0] response_word;

	wire accepted = vbuf_read && !vbuf_waitrequest;
	wire returned = vbuf_readdatavalid;
	wire request_shape_valid = vbuf_burstcount == REQUIRED_BURSTCOUNT;
	wire return_has_entry = returned && fifo_count != 2'd0;
	wire return_last = return_has_entry && return_phase == 7'd127;
	wire enqueue = accepted && request_shape_valid &&
		(fifo_count != 2'd2 || return_last);
	wire dequeue = return_last;
	wire accepted_wrap = previous_address_valid &&
		vbuf_address[27:12] < previous_address;
	wire [3:0] previous_address_fold = previous_address[3:0];
	wire [3:0] event_address_fold = accepted ?
		vbuf_address[15:12] : previous_address_fold;
	wire frozen_valid = first_stall_valid || observer_fault;

	wire [1:0] monitor_state = !reset_qualified ?
		MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_UNQUALIFIED :
		(fifo_count != 2'd0 ?
			MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_RETURN_PROGRESS :
			(vbuf_read && vbuf_waitrequest ?
				MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_ACCEPT_BLOCKED :
				MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_NO_REQUEST));
	wire expected_progress =
		(monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_RETURN_PROGRESS && returned) ||
		(monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_ACCEPT_BLOCKED && accepted) ||
		(monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_NO_REQUEST && vbuf_read);
	wire watchdog_terminal = progress_watchdog == WATCHDOG_LIMIT;
	wire request_cancel_event = reset_qualified && blocked_request_seen &&
		!vbuf_read && !accepted;
	wire bad_burst_event = accepted && !request_shape_valid;
	wire fifo_overflow_event = accepted && request_shape_valid &&
		fifo_count == 2'd2 && !return_last;
	wire unexpected_return_event = returned && fifo_count == 2'd0;
	wire snapshot_complete = snapshot_pending &&
		snapshot_response_sync == snapshot_request_toggle;
	wire snapshot_timeout_event = snapshot_pending && watchdog_terminal &&
		!snapshot_complete;
	wire observer_fault_event =
		bad_burst_event || fifo_overflow_event || unexpected_return_event ||
		snapshot_timeout_event;

	mister_magik_scaler_scheduler_snapshot scheduler_snapshot (
		.scaler_clk(scaler_clk),
		.live_state(scaler_diag_state),
		.request_toggle(snapshot_request_toggle),
		.response_toggle(snapshot_response_toggle),
		.evidence_hold(snapshot_evidence_hold)
	);

`ifdef FORMAL
	assign formal_fifo_count = fifo_count;
	assign formal_return_phase = return_phase;
	assign formal_first_stall_valid = first_stall_valid;
	assign formal_observer_fault = observer_fault;
	assign formal_frozen_state = frozen_state;
	assign formal_publication_generation = publication_generation;
	assign formal_acknowledge_sync = acknowledge_sync;
	assign formal_publication_sequence = publication_sequence;
	assign formal_publish_crc_busy = publish_crc_busy;
	assign formal_publish_crc_word = publish_crc_word;
	assign formal_published_bundle = {
		publish_crc_work,
		published_state,
		published_flags
	};
	assign formal_enqueue = enqueue;
	assign formal_dequeue = dequeue;
	assign formal_return_has_entry = return_has_entry;
	assign formal_expected_progress = expected_progress;
	assign formal_watchdog_terminal = watchdog_terminal;
	assign formal_observer_fault_event = observer_fault_event;
	assign formal_request_cancel_event = request_cancel_event;
`endif

	wire [15:0] live_flags = {
		4'd0,
		request_cancelled,
		return_incomplete,
		first_return_missing,
		accept_blocked_seen,
		no_request_seen,
		reset_since_normal_liveness,
		reset_sync,
		reset_ambiguity,
		observer_fault,
		first_stall_valid,
		normal_liveness_seen,
		ever_qualified
	};
	wire [15:0] live_state = {
		1'b0,
		address_wrap_seen,
		reset_qualified,
		(return_phase != 7'd0),
		(fifo_count != 2'd0),
		monitor_state,
		fifo_count,
		return_phase
	};
	wire [15:0] fault_frozen_state = {
		frozen_address_fold,
		frozen_fifo_depth,
		frozen_return_phase,
		frozen_cause
	};
	wire [15:0] frozen_state = no_request_seen ?
		scheduler_snapshot_state : fault_frozen_state;

	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire publication_available = generation_sync != acknowledged_generation;
	wire selected_start =
		io_din[7:0] == MAGIK_UIO_GET_SCALER_FETCH_LIVENESS_STATE &&
		publication_available;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command &&
			word_count < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS);

	function automatic [15:0] crc16_update_byte;
		input [15:0] crc_in;
		input [7:0] byte_in;
		integer bit_index;
		reg [15:0] value;
		begin
			value = crc_in ^ {byte_in, 8'h00};
			for(bit_index = 0; bit_index < 8; bit_index = bit_index + 1)
				value = value[15] ? ((value << 1) ^ 16'h1021) : (value << 1);
			crc16_update_byte = value;
		end
	endfunction

	function automatic [15:0] crc16_update_word;
		input [15:0] crc_in;
		input [15:0] word_in;
		begin
			crc16_update_word = crc16_update_byte(
				crc16_update_byte(crc_in, word_in[15:8]), word_in[7:0]);
		end
	endfunction

	always @(posedge clk_100m) begin : observe_fetch
		reg [2:0] timeout_cause;
		reset_meta <= reset_req;
		reset_sync <= reset_meta;
		reset_sync_d <= reset_sync;
		snapshot_response_meta <= snapshot_response_toggle;
		snapshot_response_sync <= snapshot_response_meta;
		acknowledge_meta <= acknowledged_generation;
		acknowledge_sync <= acknowledge_meta;

		if(reset_sync && !reset_sync_d && normal_liveness_seen)
			reset_since_normal_liveness <= 1'b1;

		if(reset_sync) begin
			reset_low_count <= 3'd0;
			progress_watchdog <= 24'd0;
			blocked_request_seen <= 1'b0;
			if(snapshot_pending)
				snapshot_invalidated <= 1'b1;
		end
		else if(!reset_qualified) begin
			if(reset_low_count + 1'd1 >= RESET_QUALIFY_LIMIT) begin
				reset_low_count <= RESET_QUALIFY_LIMIT;
				ever_qualified <= 1'b1;
			end
			else
				reset_low_count <= reset_low_count + 1'd1;
			progress_watchdog <= 24'd0;
		end
		else begin
			if(snapshot_pending) begin
				if(snapshot_complete)
					progress_watchdog <= 24'd0;
				else if(!watchdog_terminal)
					progress_watchdog <= progress_watchdog + 1'd1;
				if(expected_progress)
					snapshot_invalidated <= 1'b1;
			end
			else if(expected_progress)
				progress_watchdog <= 24'd0;
			else if(!watchdog_terminal)
				progress_watchdog <= progress_watchdog + 1'd1;

			if(fifo_count == 2'd0 && vbuf_read && vbuf_waitrequest)
				blocked_request_seen <= 1'b1;
			else if(accepted || fifo_count != 2'd0 || !vbuf_read)
				blocked_request_seen <= 1'b0;
		end

		if(accepted) begin
			if(request_shape_valid) begin
				previous_address <= vbuf_address[27:12];
				previous_address_valid <= 1'b1;
				if(accepted_wrap)
					address_wrap_seen <= 1'b1;
			end
		end

		case({enqueue, dequeue})
			2'b10: begin
				if(fifo_count == 2'd0) begin
					fifo_wrap0 <= accepted_wrap;
				end
				else begin
					fifo_wrap1 <= accepted_wrap;
				end
				fifo_count <= fifo_count + 1'd1;
			end
			2'b01: begin
				if(fifo_count == 2'd2) begin
					fifo_wrap0 <= fifo_wrap1;
				end
				fifo_count <= fifo_count - 1'd1;
			end
			2'b11: begin
				if(fifo_count == 2'd1) begin
					fifo_wrap0 <= accepted_wrap;
				end
				else begin
					fifo_wrap0 <= fifo_wrap1;
					fifo_wrap1 <= accepted_wrap;
				end
			end
			default: begin end
		endcase

		if(return_has_entry) begin
			if(return_phase == 7'd127) begin
				return_phase <= 7'd0;
				if(fifo_wrap0)
					normal_liveness_seen <= 1'b1;
			end
			else
				return_phase <= return_phase + 1'd1;
		end

		// Faults outrank cancellation and watchdog attribution. Real progress on
		// the terminal watchdog cycle wins over timeout.
		if(!first_stall_valid && !observer_fault && observer_fault_event) begin
			frozen_cause <= MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_OBSERVER_FAULT;
			frozen_return_phase <= return_phase;
			frozen_fifo_depth <= fifo_count;
			frozen_address_fold <= event_address_fold;
			if(unexpected_return_event) begin
				if(!ever_qualified)
					reset_ambiguity <= 1'b1;
			end
		end
		else if(!first_stall_valid && !observer_fault && request_cancel_event) begin
			frozen_cause <= MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_REQUEST_CANCELLED;
			frozen_return_phase <= return_phase;
			frozen_fifo_depth <= fifo_count;
			frozen_address_fold <= previous_address_fold;
		end
		else if(!first_stall_valid && !observer_fault && snapshot_complete) begin
			snapshot_pending <= 1'b0;
			progress_watchdog <= 24'd0;
			if(snapshot_invalidated || expected_progress || reset_sync) begin
				frozen_cause <= MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_OBSERVER_FAULT;
				frozen_return_phase <= return_phase;
				frozen_fifo_depth <= fifo_count;
				frozen_address_fold <= previous_address_fold;
			end
			else begin
				no_request_seen <= 1'b1;
				// The source mailbox held this complete evidence word stable before
				// its response toggle crossed into clk_100m.
				scheduler_snapshot_state <= snapshot_evidence_hold;
			end
		end
		else if(!first_stall_valid && !observer_fault && reset_qualified &&
			watchdog_terminal && !expected_progress && !snapshot_pending) begin
			if(monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_RETURN_PROGRESS)
				timeout_cause = return_phase == 7'd0 ?
					MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_FIRST_RETURN_MISSING :
					MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE;
			else if(monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_ACCEPT_BLOCKED)
				timeout_cause = MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_ACCEPT_BLOCKED;
			else
				timeout_cause = MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_NO_REQUEST_SEEN;
			if(timeout_cause == MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_NO_REQUEST_SEEN) begin
				snapshot_request_toggle <= ~snapshot_request_toggle;
				snapshot_pending <= 1'b1;
				snapshot_invalidated <= 1'b0;
				progress_watchdog <= 24'd0;
			end
			else begin
				frozen_cause <= timeout_cause;
				frozen_return_phase <= return_phase;
				frozen_fifo_depth <= fifo_count;
				frozen_address_fold <= previous_address_fold;
			end
		end

		// Capture one immutable bank and serialize its CRC before advertising it.
		if(!publish_crc_busy && publication_generation == acknowledge_sync) begin
			publication_sequence <= publication_sequence + 1'd1;
			published_flags <= {publication_sequence + 1'd1, live_flags[11:0]};
			published_state <= frozen_valid ? frozen_state : live_state;
			publish_crc_work <= MAGIK_SCALER_FETCH_LIVENESS_STATE_HEADER_CRC;
			publish_crc_word <= 2'd0;
			publish_crc_busy <= 1'b1;
		end
		else if(publish_crc_busy) begin : publish_crc_step
			reg [15:0] crc_word_value;
			reg [15:0] crc_next;
			case(publish_crc_word)
				2'd0: crc_word_value = MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA;
				2'd1: crc_word_value = published_flags;
				default: crc_word_value = published_state;
			endcase
			crc_next = crc16_update_word(publish_crc_work, crc_word_value);
			publish_crc_work <= crc_next;
			if(publish_crc_word == 2'd2) begin
				publication_generation <= ~publication_generation;
				publish_crc_busy <= 1'b0;
			end
			else
				publish_crc_word <= publish_crc_word + 1'd1;
		end
	end

	always @(*) begin
		case(word_count)
			MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA_WORD:
				response_word = MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA;
			MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAGS_WORD:
				response_word = published_flags;
			MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD:
				response_word = published_state;
			default: response_word = publish_crc_work;
		endcase

		response_data = 16'd0;
		if(command_start && selected_start)
			response_data = MAGIK_SCALER_FETCH_LIVENESS_STATE_MAGIC;
		else if(command_data && selected_command &&
			word_count < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS)
			response_data = response_word;
	end

	always @(posedge clk_sys) begin
		generation_meta <= publication_generation;
		generation_sync <= generation_meta;

		if(command_start) begin
			has_command <= 1'b1;
			command_selected <= selected_start;
			word_count <= 3'd0;
		end
		else if(command_data && selected_command &&
			word_count < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS)
			word_count <= word_count + 1'd1;

		if(!io_uio && has_command) begin
			if(command_selected)
				acknowledged_generation <= generation_sync;
			has_command <= 1'b0;
			command_selected <= 1'b0;
			word_count <= 3'd0;
		end
	end

	// Solver backends cannot model simulation-only print cells.
`ifndef FORMAL
	initial begin
		if(WATCHDOG_LIMIT != CONTRACT_WATCHDOG_LIMIT)
			$display("MagiK liveness watchdog overridden for simulation");
		if(RESET_QUALIFY_LIMIT != CONTRACT_RESET_QUALIFY_LIMIT)
			$display("MagiK liveness reset qualification overridden for simulation");
		if(CONTRACT_SNAPSHOT_HSYNC_ENTRIES != 2)
			$error("MagiK liveness snapshot contract must span two HSYNC entries");
	end
`endif

endmodule

`default_nettype wire
