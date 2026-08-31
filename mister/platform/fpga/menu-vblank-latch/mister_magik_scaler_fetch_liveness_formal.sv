// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

module mister_magik_scaler_fetch_liveness_formal;
	reg formal_clk = 1'b0;
	reg past_valid = 1'b0;
	reg drained_during_reset = 1'b0;
	reg simultaneous_event_seen = 1'b0;
	reg snapshot_request = 1'b0;
	reg snapshot_request_sent = 1'b0;
	reg snapshot_completed_seen = 1'b0;

	(* anyseq *) wire reset_req;
	(* anyseq *) wire [27:0] vbuf_address;
	(* anyseq *) wire [7:0] vbuf_burstcount;
	(* anyseq *) wire vbuf_waitrequest;
	(* anyseq *) wire vbuf_readdatavalid;
	(* anyseq *) wire vbuf_read;
	(* anyseq *) wire [15:0] scaler_diag_state;
	(* anyseq *) wire [15:0] snapshot_live_state;
	wire snapshot_response;
	wire [8:0] snapshot_evidence;
	wire snapshot_capture_active;

	wire [1:0] fifo_count;
	wire [6:0] return_phase;
	wire first_stall_valid;
	wire observer_fault;
	wire no_request_seen;
	wire snapshot_pending;
	wire terminal_record_started;
	wire [15:0] frozen_state;
	wire record_ready;
	wire [47:0] published_bundle;
	wire [3:0] publication_sequence;
	wire publish_crc_busy;
	wire [4:0] publish_crc_phase;
	wire enqueue;
	wire dequeue;
	wire return_has_entry;
	wire expected_progress;
	wire watchdog_terminal;
	wire watchdog_clear;
	wire watchdog_advance;
	wire observer_fault_event;
	wire request_cancel_event;

	mister_magik_scaler_fetch_liveness_state #(
		.WATCHDOG_LIMIT(24'd15),
		.RESET_QUALIFY_LIMIT(3'd4)
	) dut (
		.clk_100m(formal_clk),
		.clk_sys(formal_clk),
		.scaler_clk(formal_clk),
		.reset_req(reset_req),
		.vbuf_address(vbuf_address),
		.vbuf_burstcount(vbuf_burstcount),
		.vbuf_waitrequest(vbuf_waitrequest),
		.vbuf_readdatavalid(vbuf_readdatavalid),
		.vbuf_read(vbuf_read),
		.scaler_diag_state(scaler_diag_state),
		.io_uio(1'b0),
		.io_strobe(1'b0),
		.io_din(16'd0),
		.response_valid(),
		.response_data(),
		.formal_fifo_count(fifo_count),
		.formal_return_phase(return_phase),
		.formal_first_stall_valid(first_stall_valid),
		.formal_observer_fault(observer_fault),
		.formal_no_request_seen(no_request_seen),
		.formal_snapshot_pending(snapshot_pending),
		.formal_terminal_record_started(terminal_record_started),
		.formal_frozen_state(frozen_state),
		.formal_record_ready(record_ready),
		.formal_published_bundle(published_bundle),
		.formal_publication_sequence(publication_sequence),
		.formal_publish_crc_busy(publish_crc_busy),
		.formal_publish_crc_phase(publish_crc_phase),
		.formal_enqueue(enqueue),
		.formal_dequeue(dequeue),
		.formal_return_has_entry(return_has_entry),
		.formal_expected_progress(expected_progress),
		.formal_watchdog_terminal(watchdog_terminal),
		.formal_watchdog_clear(watchdog_clear),
		.formal_watchdog_advance(watchdog_advance),
		.formal_observer_fault_event(observer_fault_event),
		.formal_request_cancel_event(request_cancel_event)
	);

	mister_magik_scaler_scheduler_snapshot snapshot_proof (
		.scaler_clk(formal_clk),
		.live_state(snapshot_live_state),
		.request_toggle(snapshot_request),
		.response_toggle(snapshot_response),
		.evidence_hold(snapshot_evidence),
		.formal_capture_active(snapshot_capture_active)
	);

	always @($global_clock)
		formal_clk <= !formal_clk;

	always @(posedge formal_clk) begin
		past_valid <= 1'b1;
		if(!snapshot_request_sent) begin
			snapshot_request <= 1'b1;
			snapshot_request_sent <= 1'b1;
		end
		if(snapshot_response)
			snapshot_completed_seen <= 1'b1;
		assert(fifo_count <= 2);
		assert(return_phase < 128);
		assert(!no_request_seen || !snapshot_pending);
		assert(!record_ready || terminal_record_started);
		assert(!record_ready || !publish_crc_busy);
		assert(!publish_crc_busy || terminal_record_started);
		assert(!publish_crc_busy || !record_ready);
		assert(!publish_crc_busy || publish_crc_phase <= 5'd30);
		assert(!terminal_record_started || first_stall_valid || observer_fault);
		assert(!watchdog_clear || !watchdog_advance);

		if(past_valid) begin
			if(snapshot_response != $past(snapshot_response)) begin
				assert($past(snapshot_capture_active));
			end
			if(snapshot_response == $past(snapshot_response) &&
				!$past(snapshot_capture_active) &&
				$past(snapshot_request == snapshot_response)) begin
				assert(snapshot_evidence == $past(snapshot_evidence));
			end
			case({$past(enqueue), $past(dequeue)})
				2'b10: assert(fifo_count == $past(fifo_count) + 1'b1);
				2'b01: assert(fifo_count + 1'b1 == $past(fifo_count));
				default: assert(fifo_count == $past(fifo_count));
			endcase

			if($past(return_has_entry)) begin
				if($past(return_phase) == 127)
					assert(return_phase == 0);
				else
					assert(return_phase == $past(return_phase) + 1'b1);
			end
			else
				assert(return_phase == $past(return_phase));

			if($past(first_stall_valid)) begin
				assert(first_stall_valid);
				assert(frozen_state == $past(frozen_state));
			end
			if($past(observer_fault)) begin
				assert(observer_fault);
				assert(frozen_state == $past(frozen_state));
			end
			assert(publication_sequence == 4'd0);
			if($past(publish_crc_busy))
				assert(published_bundle[31:0] == $past(published_bundle[31:0]));
			if($past(record_ready)) begin
				assert(record_ready);
				assert(published_bundle == $past(published_bundle));
			end
			if(record_ready != $past(record_ready)) begin
				assert($past(publish_crc_busy));
				assert($past(publish_crc_phase) == 5'd30);
			end
			// A real transition on the terminal watchdog cycle wins unless an
			// independently higher-priority observer event occurred.
			if($past(watchdog_terminal && expected_progress &&
				!first_stall_valid && !observer_fault &&
				!observer_fault_event && !request_cancel_event))
				assert(!first_stall_valid);

		end

		if(reset_req && fifo_count != 0 && return_has_entry)
			drained_during_reset <= 1'b1;
		if(enqueue && dequeue)
			simultaneous_event_seen <= 1'b1;
		cover(drained_during_reset);
		cover(first_stall_valid);
		cover(observer_fault);
		cover(enqueue && dequeue);
		cover(snapshot_completed_seen);
	end
endmodule
