// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

`timescale 1ns/1ps
`default_nettype none

// Source-clock evidence collector and closed-loop multi-cycle-path snapshot.
// Capture aligns to one complete HSYNC-to-HSYNC scheduler revolution. Six
// one-hot outcomes plus three independent sidebands cross bit-for-bit to the
// destination. The held semantic record is immutable before the response
// handoff crosses to the destination.
module mister_magik_scaler_scheduler_snapshot (
	input  wire        scaler_clk,
	input  wire [15:0] live_state,
	input  wire        request_toggle,
	output wire        response_toggle,
	output wire [8:0]  evidence_hold
`ifdef FORMAL
	,output wire        formal_capture_active
`endif
);
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg request_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg request_sync = 1'b0;
	reg capture_active = 1'b0;
	reg window_started = 1'b0;
	reg [1:0] previous_output_state = 2'd0;
	reg previous_vertical_pixel_enable = 1'b0;
	reg previous_vertical_carry = 1'b0;
	(* preserve, dont_replicate *) reg response_state = 1'b0;
	(* preserve, dont_replicate *) reg response_handoff_bit = 1'b0;
	(* preserve, dont_replicate *) reg [8:0] semantic_evidence = 9'd0;

	wire [1:0] output_state = live_state[1:0];
	wire hsync_entry = previous_output_state != 2'd1 && output_state == 2'd1;
	wire left_hsync = previous_output_state == 2'd1 && output_state != 2'd1;
	reg [8:0] next_semantic_evidence;
	always @(*) begin
		next_semantic_evidence = semantic_evidence;
		next_semantic_evidence[6] = semantic_evidence[6] || live_state[2];
		next_semantic_evidence[7] = semantic_evidence[7] ||
			(previous_output_state == 2'd1 && output_state == 2'd1);
		next_semantic_evidence[8] = semantic_evidence[8] || live_state[12];
		if(previous_output_state == 2'd2 && output_state == 2'd3)
			next_semantic_evidence[5:0] = 6'b100000;
		else if(output_state == 2'd2 && live_state[8])
			next_semantic_evidence[5:0] = 6'b010000;
		else if(output_state == 2'd2)
			next_semantic_evidence[5:0] = 6'b001000;
		else if(left_hsync && output_state == 2'd0) begin
			case({!previous_vertical_carry, !previous_vertical_pixel_enable})
				2'b01: next_semantic_evidence[5:0] = 6'b000001;
				2'b10: next_semantic_evidence[5:0] = 6'b000010;
				2'b11: next_semantic_evidence[5:0] = 6'b000100;
				default: next_semantic_evidence[5:0] = 6'd0;
			endcase
		end
	end

	assign response_toggle = response_handoff_bit;
	assign evidence_hold = semantic_evidence;

`ifdef FORMAL
	assign formal_capture_active = capture_active;
`endif

	always @(posedge scaler_clk) begin
		request_meta <= request_toggle;
		request_sync <= request_meta;
		previous_output_state <= output_state;
		previous_vertical_pixel_enable <= live_state[5];
		previous_vertical_carry <= live_state[6];

		if(request_sync == response_state) begin
			capture_active <= 1'b0;
			window_started <= 1'b0;
		end
		else if(!capture_active) begin
			capture_active <= 1'b1;
			window_started <= 1'b0;
		end
		else if(!window_started) begin
			if(hsync_entry) begin
				window_started <= 1'b1;
				semantic_evidence <= {
					live_state[12],
					1'b0,
					live_state[2],
					6'd0
				};
			end
		end
		else if(hsync_entry) begin
			semantic_evidence <= next_semantic_evidence;
			response_state <= request_sync;
			response_handoff_bit <= request_sync;
			capture_active <= 1'b0;
			window_started <= 1'b0;
		end
		else begin
			semantic_evidence <= next_semantic_evidence;
		end
	end
endmodule

// Replacement-only passive observer at the external scaler Avalon boundary.
// reset_req is observed as data: it never clears accepted return obligations,
// telemetry, or the terminal record. A no-request timeout freezes the
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
	,output wire formal_no_request_seen
	,output wire formal_snapshot_pending
	,output wire formal_terminal_record_started
	,output wire [15:0] formal_frozen_state
	,output wire formal_record_ready
	,output wire [47:0] formal_published_bundle
	,output wire [3:0] formal_publication_sequence
	,output wire formal_publish_crc_busy
	,output wire [4:0] formal_publish_crc_phase
	,output wire [15:0] formal_publish_crc_input
	,output wire formal_enqueue
	,output wire formal_dequeue
	,output wire formal_return_has_entry
	,output wire formal_expected_progress
	,output wire formal_watchdog_terminal
	,output wire formal_watchdog_clear
	,output wire formal_watchdog_advance
	,output wire formal_observer_fault_event
	,output wire formal_request_cancel_event
	,output wire [3:0] formal_terminal_cause
	,output wire [15:0] formal_terminal_flags
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
	localparam [2:0] COMPACT_CAUSE_NONE =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_NONE[2:0];
	localparam [2:0] COMPACT_CAUSE_NO_REQUEST_SEEN =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_NO_REQUEST_SEEN[2:0];
	localparam [2:0] COMPACT_CAUSE_ACCEPT_BLOCKED =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_ACCEPT_BLOCKED[2:0];
	localparam [2:0] COMPACT_CAUSE_FIRST_RETURN_MISSING =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_FIRST_RETURN_MISSING[2:0];
	localparam [2:0] COMPACT_CAUSE_RETURN_INCOMPLETE =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_RETURN_INCOMPLETE[2:0];
	localparam [2:0] COMPACT_CAUSE_REQUEST_CANCELLED =
		MAGIK_SCALER_FETCH_LIVENESS_STATE_CAUSE_REQUEST_CANCELLED[2:0];

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
	reg [1:0] fifo_count = 2'd0;
	reg [6:0] return_phase = 7'd0;

	reg normal_liveness_seen = 1'b0;
	reg blocked_request_seen = 1'b0;
	reg [23:0] progress_watchdog = 24'd0;
	reg snapshot_request_toggle = 1'b0;
	reg snapshot_pending = 1'b0;
	reg snapshot_reset_invalidated = 1'b0;
	reg snapshot_progress_invalidated = 1'b0;
	wire snapshot_response_toggle;
	wire [8:0] snapshot_evidence_hold;
	(* preserve, dont_replicate *) reg [8:0] scheduler_snapshot_capture = 9'd0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg snapshot_response_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg snapshot_response_sync = 1'b0;

	// Sticky first-stall/fault evidence. The first terminal record is immutable
	// and becomes command-visible only after its CRC is complete.
	reg reset_ambiguity = 1'b0;
	reg reset_since_normal_liveness = 1'b0;
	reg no_request_seen = 1'b0;
	// Scheduler evidence and Avalon terminal context use separate immutable
	// banks. This keeps the asynchronous payload destination single-source and
	// leaves all terminal selection in the local clock domain.
	reg [1:0] avalon_terminal_fifo_depth = 2'd0;
	reg [6:0] avalon_terminal_return_phase = 7'd0;
	// Keep the observer namespace sticky bit independent from the three-bit
	// normal-cause/subcause bank. The compact state word publishes only the
	// bank, while the flags word publishes the namespace bit.
	reg [2:0] avalon_terminal_cause = 3'd0;
	reg observer_fault = 1'b0;
	wire [2:0] frozen_cause = avalon_terminal_cause;
	wire first_stall_valid = no_request_seen ||
		(!observer_fault && frozen_cause != COMPACT_CAUSE_NONE);
	reg terminal_reset_level = 1'b1;
	reg terminal_record_started = 1'b0;
	(* preserve, dont_replicate *) reg record_ready = 1'b0;
	// The schema is constant-folded into the generated seed; capture folds
	// flags[15], then phases 0..30 fold the remaining 31 bits. A separate busy
	// ownership bit makes completed-bank immutability a local invariant.
	reg publish_crc_busy = 1'b0;
	reg [4:0] publish_crc_phase = 5'd0;
	reg [15:0] publish_crc_work = 16'd0;
	// Fold the immutable payload MSB-first without a phase-indexed selector.
	// flags[15] is consumed on launch; this register supplies flags[14:0],
	// then reloads once to supply state[15:0].
	reg [15:0] publish_crc_input = 16'd0;

	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED" *)
	reg record_ready_meta = 1'b0;
	(* altera_attribute = "-name SYNCHRONIZER_IDENTIFICATION FORCED_IF_ASYNCHRONOUS" *)
	reg record_ready_sync = 1'b0;

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
	wire [2:0] observer_fault_subcause = bad_burst_event ?
		MAGIK_SCALER_FETCH_LIVENESS_STATE_OBSERVER_SUBCAUSE_BAD_BURST :
		(fifo_overflow_event ?
			MAGIK_SCALER_FETCH_LIVENESS_STATE_OBSERVER_SUBCAUSE_FIFO_OVERFLOW :
			(unexpected_return_event ?
				MAGIK_SCALER_FETCH_LIVENESS_STATE_OBSERVER_SUBCAUSE_ORPHAN_RETURN :
				MAGIK_SCALER_FETCH_LIVENESS_STATE_OBSERVER_SUBCAUSE_SNAPSHOT_TIMEOUT));
	wire [5:0] scheduler_snapshot_outcomes = scheduler_snapshot_capture[5:0];
	wire scheduler_snapshot_outcome_valid =
		scheduler_snapshot_outcomes != 6'd0 &&
		(scheduler_snapshot_outcomes & (scheduler_snapshot_outcomes - 1'd1)) == 6'd0;

	mister_magik_scaler_scheduler_snapshot scheduler_snapshot (
		.scaler_clk(scaler_clk),
		.live_state(scaler_diag_state),
		.request_toggle(snapshot_request_toggle),
		.response_toggle(snapshot_response_toggle),
		.evidence_hold(snapshot_evidence_hold)
	);

	wire [15:0] scheduler_terminal_state = {7'd0, scheduler_snapshot_capture};
	wire [15:0] avalon_terminal_state = {
		4'd0,
		avalon_terminal_fifo_depth,
		avalon_terminal_return_phase,
		avalon_terminal_cause[2:0]
	};
	wire [15:0] frozen_state = no_request_seen ?
		scheduler_terminal_state : avalon_terminal_state;
	wire terminal_valid = first_stall_valid || observer_fault;
	wire watchdog_snapshot_start = !terminal_valid && !observer_fault_event &&
		!request_cancel_event && reset_qualified && watchdog_terminal &&
		!expected_progress && !snapshot_pending &&
		monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_NO_REQUEST;
	wire watchdog_clear = reset_sync || !reset_qualified || snapshot_complete ||
		(!snapshot_pending && expected_progress) || watchdog_snapshot_start;
	wire watchdog_advance = !watchdog_clear && !watchdog_terminal &&
		(snapshot_pending || !expected_progress);
	wire [15:0] terminal_flags = {
		8'd0,
		no_request_seen,
		reset_since_normal_liveness,
		terminal_reset_level,
		reset_ambiguity,
		observer_fault,
		first_stall_valid,
		normal_liveness_seen,
		ever_qualified
	};

`ifdef FORMAL
	assign formal_fifo_count = fifo_count;
	assign formal_return_phase = return_phase;
	assign formal_first_stall_valid = first_stall_valid;
	assign formal_observer_fault = observer_fault;
	assign formal_no_request_seen = no_request_seen;
	assign formal_snapshot_pending = snapshot_pending;
	assign formal_terminal_record_started = terminal_record_started;
	assign formal_frozen_state = frozen_state;
	assign formal_record_ready = record_ready;
	assign formal_publication_sequence = 4'd0;
	assign formal_publish_crc_busy = publish_crc_busy;
	assign formal_publish_crc_phase = publish_crc_phase;
	assign formal_publish_crc_input = publish_crc_input;
	assign formal_published_bundle = {
		publish_crc_work,
		frozen_state,
		terminal_flags
	};
	assign formal_enqueue = enqueue;
	assign formal_dequeue = dequeue;
	assign formal_return_has_entry = return_has_entry;
	assign formal_expected_progress = expected_progress;
	assign formal_watchdog_terminal = watchdog_terminal;
	assign formal_watchdog_clear = watchdog_clear;
	assign formal_watchdog_advance = watchdog_advance;
	assign formal_observer_fault_event = observer_fault_event;
	assign formal_request_cancel_event = request_cancel_event;
	assign formal_terminal_cause = {observer_fault, avalon_terminal_cause};
	assign formal_terminal_flags = terminal_flags;
`endif

	wire command_start = io_uio && io_strobe && !has_command;
	wire command_data = io_uio && io_strobe && has_command;
	wire selected_start =
		io_din[7:0] == MAGIK_UIO_GET_SCALER_FETCH_LIVENESS_STATE &&
		record_ready_sync;
	wire selected_command = command_selected;

	assign response_valid =
		(command_start && selected_start) ||
		(command_data && selected_command &&
			word_count < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS);

	function automatic [15:0] crc16_update_bit;
		input [15:0] crc_in;
		input bit_in;
		reg [15:0] value;
		begin
			value = {crc_in[14:0], 1'b0};
			if(crc_in[15] ^ bit_in)
				value = value ^ 16'h1021;
			crc16_update_bit = value;
		end
	endfunction

	always @(posedge clk_100m) begin : observe_fetch
		reg [2:0] timeout_cause;
		reset_meta <= reset_req;
		reset_sync <= reset_meta;
		reset_sync_d <= reset_sync;
		snapshot_response_meta <= snapshot_response_toggle;
		snapshot_response_sync <= snapshot_response_meta;
		if(watchdog_clear)
			progress_watchdog <= 24'd0;
		else if(watchdog_advance)
			progress_watchdog <= progress_watchdog + 1'd1;
		if(snapshot_pending)
			scheduler_snapshot_capture <= snapshot_evidence_hold;
		if(!terminal_valid)
			terminal_reset_level <= reset_sync;

		if(!terminal_valid && reset_sync && !reset_sync_d && normal_liveness_seen)
			reset_since_normal_liveness <= 1'b1;

		if(reset_sync) begin
			reset_low_count <= 3'd0;
			blocked_request_seen <= 1'b0;
			if(snapshot_pending)
				snapshot_reset_invalidated <= 1'b1;
		end
		else if(!reset_qualified) begin
			if(!terminal_valid) begin
				if(reset_low_count + 1'd1 >= RESET_QUALIFY_LIMIT) begin
					reset_low_count <= RESET_QUALIFY_LIMIT;
					ever_qualified <= 1'b1;
				end
				else
					reset_low_count <= reset_low_count + 1'd1;
			end
		end
		else begin
			if(snapshot_pending && expected_progress)
				snapshot_progress_invalidated <= 1'b1;

			if(fifo_count == 2'd0 && vbuf_read && vbuf_waitrequest)
				blocked_request_seen <= 1'b1;
			else if(accepted || fifo_count != 2'd0 || !vbuf_read)
				blocked_request_seen <= 1'b0;
		end

		case({enqueue, dequeue})
			2'b10: begin
				fifo_count <= fifo_count + 1'd1;
			end
			2'b01: begin
				fifo_count <= fifo_count - 1'd1;
			end
			2'b11: begin end
			default: begin end
		endcase

		if(return_has_entry) begin
			if(return_phase == 7'd127) begin
				return_phase <= 7'd0;
				if(!terminal_valid)
					normal_liveness_seen <= 1'b1;
			end
			else
				return_phase <= return_phase + 1'd1;
		end

		// Faults outrank cancellation and watchdog attribution. Real progress on
		// the terminal watchdog cycle wins over timeout.
		if(!first_stall_valid && !observer_fault && observer_fault_event) begin
			avalon_terminal_fifo_depth <= fifo_count;
			avalon_terminal_return_phase <= return_phase;
			observer_fault <= 1'b1;
			avalon_terminal_cause <= observer_fault_subcause;
			if(unexpected_return_event) begin
				if(!ever_qualified)
					reset_ambiguity <= 1'b1;
			end
		end
		else if(!first_stall_valid && !observer_fault && request_cancel_event) begin
			avalon_terminal_fifo_depth <= fifo_count;
			avalon_terminal_return_phase <= return_phase;
			avalon_terminal_cause <= COMPACT_CAUSE_REQUEST_CANCELLED;
		end
		else if(!first_stall_valid && !observer_fault && snapshot_complete) begin
			snapshot_pending <= 1'b0;
			if(snapshot_reset_invalidated || reset_sync ||
				snapshot_progress_invalidated || expected_progress ||
				!scheduler_snapshot_outcome_valid) begin
				avalon_terminal_fifo_depth <= fifo_count;
				avalon_terminal_return_phase <= return_phase;
				observer_fault <= 1'b1;
				if(snapshot_reset_invalidated || reset_sync)
					avalon_terminal_cause <=
						MAGIK_SCALER_FETCH_LIVENESS_STATE_OBSERVER_SUBCAUSE_SNAPSHOT_RESET_INVALIDATED;
				else if(snapshot_progress_invalidated || expected_progress)
					avalon_terminal_cause <=
						MAGIK_SCALER_FETCH_LIVENESS_STATE_OBSERVER_SUBCAUSE_SNAPSHOT_PROGRESS_INVALIDATED;
				else
					avalon_terminal_cause <=
						MAGIK_SCALER_FETCH_LIVENESS_STATE_OBSERVER_SUBCAUSE_SNAPSHOT_INVALID_OUTCOME;
			end
			else begin
				no_request_seen <= 1'b1;
			end
		end
		else if(!first_stall_valid && !observer_fault && reset_qualified &&
			watchdog_terminal && !expected_progress && !snapshot_pending) begin
			if(monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_RETURN_PROGRESS)
				timeout_cause = return_phase == 7'd0 ?
					COMPACT_CAUSE_FIRST_RETURN_MISSING :
					COMPACT_CAUSE_RETURN_INCOMPLETE;
			else if(monitor_state == MAGIK_SCALER_FETCH_LIVENESS_STATE_MONITOR_ACCEPT_BLOCKED)
				timeout_cause = COMPACT_CAUSE_ACCEPT_BLOCKED;
			else
				timeout_cause = COMPACT_CAUSE_NO_REQUEST_SEEN;
			if(timeout_cause == COMPACT_CAUSE_NO_REQUEST_SEEN) begin
				snapshot_request_toggle <= ~snapshot_request_toggle;
				snapshot_pending <= 1'b1;
				snapshot_reset_invalidated <= 1'b0;
				snapshot_progress_invalidated <= 1'b0;
			end
			else begin
				avalon_terminal_fifo_depth <= fifo_count;
				avalon_terminal_return_phase <= return_phase;
				avalon_terminal_cause <= timeout_cause;
			end
		end

		// Serialize the first immutable terminal record once, then advertise it.
		if(!terminal_record_started && terminal_valid) begin
			terminal_record_started <= 1'b1;
			publish_crc_work <= crc16_update_bit(
				MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA_CRC,
				terminal_flags[15]);
			publish_crc_input <= {terminal_flags[14:0], 1'b0};
			publish_crc_busy <= 1'b1;
			publish_crc_phase <= 5'd0;
		end
		else if(publish_crc_busy) begin : publish_crc_step
			reg crc_data_bit;
			reg [15:0] crc_next;
			crc_data_bit = publish_crc_input[15];
			crc_next = crc16_update_bit(publish_crc_work, crc_data_bit);
			publish_crc_work <= crc_next;
			if(publish_crc_phase == 5'd30) begin
				publish_crc_busy <= 1'b0;
				record_ready <= 1'b1;
			end
			else begin
				if(publish_crc_phase == 5'd14)
					publish_crc_input <= frozen_state;
				else
					publish_crc_input <= {publish_crc_input[14:0], 1'b0};
				publish_crc_phase <= publish_crc_phase + 1'd1;
			end
		end
	end

	always @(*) begin
		case(word_count)
			MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA_WORD:
				response_word = MAGIK_SCALER_FETCH_LIVENESS_STATE_SCHEMA;
			MAGIK_SCALER_FETCH_LIVENESS_STATE_FLAGS_WORD:
				response_word = terminal_flags;
			MAGIK_SCALER_FETCH_LIVENESS_STATE_STATE_WORD:
				response_word = frozen_state;
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
		record_ready_meta <= record_ready;
		record_ready_sync <= record_ready_meta;

		if(command_start) begin
			has_command <= 1'b1;
			command_selected <= selected_start;
			word_count <= 3'd0;
		end
		else if(command_data && selected_command &&
			word_count < MAGIK_SCALER_FETCH_LIVENESS_STATE_WORDS)
			word_count <= word_count + 1'd1;

		if(!io_uio && has_command) begin
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
