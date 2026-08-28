// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

module mister_magik_scaler_fetch_liveness_formal;
	reg formal_clk = 1'b0;
	reg past_valid = 1'b0;
	reg drained_during_reset = 1'b0;
	reg simultaneous_event_seen = 1'b0;

	(* anyseq *) wire reset_req;
	(* anyseq *) wire [27:0] vbuf_address;
	(* anyseq *) wire [7:0] vbuf_burstcount;
	(* anyseq *) wire vbuf_waitrequest;
	(* anyseq *) wire vbuf_readdatavalid;
	(* anyseq *) wire vbuf_read;

	wire [1:0] fifo_count;
	wire [6:0] return_phase;
	wire first_stall_valid;
	wire observer_fault;
	wire [15:0] frozen_state;
	wire publication_generation;
	wire acknowledge_sync;
	wire [47:0] published_bundle;
	wire [3:0] publication_sequence;
	wire publish_crc_busy;
	wire [1:0] publish_crc_word;
	wire enqueue;
	wire dequeue;
	wire return_has_entry;
	wire expected_progress;
	wire watchdog_terminal;
	wire observer_fault_event;
	wire request_cancel_event;

	mister_magik_scaler_fetch_liveness_state #(
		.WATCHDOG_LIMIT(24'd7),
		.RESET_QUALIFY_LIMIT(3'd4)
	) dut (
		.clk_100m(formal_clk),
		.clk_sys(formal_clk),
		.reset_req(reset_req),
		.vbuf_address(vbuf_address),
		.vbuf_burstcount(vbuf_burstcount),
		.vbuf_waitrequest(vbuf_waitrequest),
		.vbuf_readdatavalid(vbuf_readdatavalid),
		.vbuf_read(vbuf_read),
		.io_uio(1'b0),
		.io_strobe(1'b0),
		.io_din(16'd0),
		.response_valid(),
		.response_data(),
		.formal_fifo_count(fifo_count),
		.formal_return_phase(return_phase),
		.formal_first_stall_valid(first_stall_valid),
		.formal_observer_fault(observer_fault),
		.formal_frozen_state(frozen_state),
		.formal_publication_generation(publication_generation),
		.formal_acknowledge_sync(acknowledge_sync),
		.formal_published_bundle(published_bundle),
		.formal_publication_sequence(publication_sequence),
		.formal_publish_crc_busy(publish_crc_busy),
		.formal_publish_crc_word(publish_crc_word),
		.formal_enqueue(enqueue),
		.formal_dequeue(dequeue),
		.formal_return_has_entry(return_has_entry),
		.formal_expected_progress(expected_progress),
		.formal_watchdog_terminal(watchdog_terminal),
		.formal_observer_fault_event(observer_fault_event),
		.formal_request_cancel_event(request_cancel_event)
	);

	always @($global_clock)
		formal_clk <= !formal_clk;

	always @(posedge formal_clk) begin
		past_valid <= 1'b1;
		assert(fifo_count <= 2);
		assert(return_phase < 128);

		if(past_valid) begin
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
			if($past(publication_generation != acknowledge_sync))
				assert(published_bundle == $past(published_bundle));
			if(publication_sequence != $past(publication_sequence)) begin
				assert($past(publication_generation == acknowledge_sync));
				assert(!$past(publish_crc_busy));
				assert(publication_sequence == $past(publication_sequence) + 1'b1);
			end
			if(publication_generation != $past(publication_generation)) begin
				assert($past(publish_crc_busy));
				assert($past(publish_crc_word) == 2'd2);
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
	end
endmodule
