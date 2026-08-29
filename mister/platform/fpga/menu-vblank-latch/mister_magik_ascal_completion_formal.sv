// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

// Formal environment for the GHDL-synthesized narrow production scheduler.
// The Avalon responder scoreboard is deliberately independent of DUT return
// accounting and survives reset. Symbolic avl_step/o_step permits every clock
// ordering, coincidence, pause, and independently synchronized reset release.

module mister_magik_ascal_completion_formal;
	localparam integer BLEN = 128;
	localparam integer MAX_WORDS = 2 * BLEN;

	reg formal_clk = 1'b0;
	reg past_valid = 1'b0;
	reg [8:0] reference_words = 9'd0;
	reg [2:0] unseen_completions_state = 3'd0;
	reg [6:0] visible_beat = 7'd0;
	reg first_post_drain_active = 1'b0;
	reg [1:0] stopped_completion_count = 2'd0;
	reg [1:0] resumed_delivery_count = 2'd0;
	reg saw_two_stopped = 1'b0;
	reg saw_reset_obligation = 1'b0;
	reg [9:0] witness_cycle = 10'd0;
	reg [3:0] witness_phase = 4'd0;

	reg cover_two_stopped_delivered = 1'b0;
	reg cover_coincident_ack_completion = 1'b0;
	reg cover_final_old_beat_during_reset = 1'b0;
	reg cover_old_beat_after_reset = 1'b0;
	reg cover_vs_alignment_during_drain = 1'b0;
	reg cover_drain_release_without_vs = 1'b0;
	reg cover_first_post_drain_completion = 1'b0;
	reg cover_active_credit_vs = 1'b0;
	reg cover_issue_empty_vs = 1'b0;
	reg cover_final_return_vs_wait = 1'b0;

	(* anyseq *) wire reset_n;
	(* anyseq *) wire avl_step;
	(* anyseq *) wire o_step;
	(* anyseq *) wire waitrequest;
	(* anyseq *) wire return_valid;
	(* anyseq *) wire vs_edge;
	(* anyseq *) wire schedule_read;
	(* anyseq *) wire request_copy_retire;

	wire avl_reset_n;
	wire o_reset_n;
	wire issue_event;
	wire read_assert_event;
	wire return_event;
	wire write_event;
	wire completion_event;
	wire align_event;
	wire release_event;
	wire release_pending;
	wire read_start_event;
	wire copy_retire_event;
	wire completion_seen;
	wire queue_overflow;
	wire accounting_invalid;
	wire return_drain;
	wire [1:0] return_credits;
	wire [6:0] return_phase;
	wire [8:0] words_remaining;
	wire [7:0] write_phase;
	wire request_toggle;
	wire completion_pending;
	wire request_meta;
	wire request_sync;
	wire completion_pulse;
	wire ack_meta;
	wire ack_sync;
	wire [1:0] read_pending;
	wire read_active;
	wire read_accepted;
	wire [1:0] readlev;
	wire [1:0] copylev;

	wire proof_edge = !formal_clk;
	wire [1:0] unseen_queue_depth =
		{1'b0, request_toggle != request_sync} +
		{1'b0, completion_pending};
	wire [2:0] unseen_completions =
		(!avl_reset_n || !o_reset_n) ? 3'd0 : unseen_completions_state;
	wire [3:0] scheduler_obligations =
		{2'b00, return_credits} +
		{2'b00, unseen_queue_depth} +
		{3'b000, completion_pulse} +
		{2'b00, copylev} +
		{2'b00, read_pending} +
		{3'b000, read_active && !read_accepted};
	wire [2:0] output_obligations =
		{2'b00, completion_pulse} + {1'b0, copylev};

	mister_magik_scaler_completion_formal_dut dut (
		.clk(formal_clk),
		.reset_n(reset_n),
		.avl_step(avl_step),
		.o_step(o_step),
		.waitrequest(waitrequest),
		.return_valid(return_valid),
		.vs_edge(vs_edge),
		.schedule_read(schedule_read),
		.request_copy_retire(request_copy_retire),
		.avl_reset_n_o(avl_reset_n),
		.o_reset_n_o(o_reset_n),
		.issue_event_o(issue_event),
		.read_assert_event_o(read_assert_event),
		.return_event_o(return_event),
		.write_event_o(write_event),
		.completion_event_o(completion_event),
		.align_event_o(align_event),
		.release_event_o(release_event),
		.release_pending_o(release_pending),
		.read_start_event_o(read_start_event),
		.copy_retire_event_o(copy_retire_event),
		.completion_seen_o(completion_seen),
		.queue_overflow_o(queue_overflow),
		.accounting_invalid_o(accounting_invalid),
		.return_drain_o(return_drain),
		.return_credits_o(return_credits),
		.return_phase_o(return_phase),
		.words_remaining_o(words_remaining),
		.write_phase_o(write_phase),
		.request_toggle_o(request_toggle),
		.completion_pending_o(completion_pending),
		.request_meta_o(request_meta),
		.request_sync_o(request_sync),
		.completion_pulse_o(completion_pulse),
		.ack_meta_o(ack_meta),
		.ack_sync_o(ack_sync),
		.read_pending_o(read_pending),
		.read_active_o(read_active),
		.read_accepted_o(read_accepted),
		.readlev_o(readlev),
		.copylev_o(copylev)
	);

	always @($global_clock) begin
		formal_clk <= !formal_clk;
		past_valid <= 1'b1;
		if (!past_valid)
			assume(!reset_n);

		// These are state invariants, so constrain both half-cycles used to
		// encode the formal clock. This gives induction the same production
		// conservation facts that hold between real rising edges.
		assert(words_remaining == reference_words);
		assert(words_remaining <= MAX_WORDS);
		assert(return_credits <= 2);
		assert(return_phase < BLEN);
		assert(write_phase < MAX_WORDS);
		assert(readlev <= 2);
		assert(copylev <= 2);
		assert(read_pending <= 2);
		assert(read_pending <= readlev);
		if (release_pending) begin
			assert(return_drain);
			assert(words_remaining == 0);
		end
		if (request_toggle == request_sync)
			assert(request_meta == request_sync);
		if (completion_pulse)
			assert(request_meta == request_sync);
		if (request_sync == ack_sync)
			assert(ack_meta == ack_sync);
		if (request_toggle == ack_sync)
			assert(request_sync == request_toggle);
		if (return_credits == 0)
			assert(return_phase == 0);
		assert(unseen_completions ==
			{1'b0, unseen_queue_depth} +
			{2'b00, completion_pulse});
		if (o_reset_n)
			assert(output_obligations <= {1'b0, readlev});
		if (avl_reset_n && o_reset_n && !return_drain)
			assert(scheduler_obligations <= {2'b00, readlev});
		if (avl_reset_n && !return_drain) begin
			assert(write_phase[6:0] + 1'b1 == return_phase);
			assert(visible_beat == return_phase);
		end
		if (!avl_reset_n) begin
			assert(!request_toggle && !completion_pending);
			assert(!ack_meta && !ack_sync);
			assert(return_drain);
		end
		if (!o_reset_n) begin
			assert(!request_meta && !request_sync && !completion_pulse);
			assert(readlev == 0 && copylev == 0 && read_pending == 0);
		end
		if (!avl_reset_n || !o_reset_n)
			assert(unseen_completions == 0);
		if (return_drain) begin
			assert(unseen_completions == 0);
			assert(!read_active);
			assert(!request_toggle && !completion_pending);
			assert(!request_meta && !request_sync && !completion_pulse);
			assert(copylev == 0);
		end

		if (proof_edge) begin
			// Cover-only builds select one deterministic legal witness. These
			// assumptions are never compiled into the safety proof.
`ifdef COVER_WITNESS_TWO_STOPPED
			witness_cycle <= witness_cycle + 1'b1;
			assume(reset_n == (witness_cycle != 0));
			assume(avl_step && !waitrequest && !request_copy_retire);
			assume(vs_edge == (witness_cycle == 2));
			assume(schedule_read ==
				(witness_cycle == 2 || witness_cycle == 3));
			assume(return_valid ==
				(witness_cycle >= 5 && witness_cycle <= 260));
			assume(o_step ==
				!(witness_cycle >= 5 && witness_cycle <= 260));
`elsif COVER_WITNESS_COINCIDENT
			witness_cycle <= witness_cycle + 1'b1;
			assume(reset_n == (witness_cycle != 0));
			assume(avl_step && o_step && !waitrequest && !request_copy_retire);
			assume(vs_edge == (witness_cycle == 2));
			assume(schedule_read ==
				(witness_cycle == 2 || witness_cycle == 3));
			assume(return_valid ==
				(witness_cycle >= 5 && witness_cycle <= 260));
`elsif COVER_WITNESS_FINAL_RESET
			assume(reset_n == (witness_phase != 0 && witness_phase != 4));
			assume(avl_step && o_step && !waitrequest && !request_copy_retire);
			assume(vs_edge == (witness_phase == 1 && avl_reset_n &&
				return_drain && reference_words == 0));
			assume(schedule_read == (witness_phase == 2));
			assume(return_valid ==
				(witness_phase == 4 && reference_words != 0));
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
				2: if (read_start_event) witness_phase <= 3;
				3: if (issue_event) witness_phase <= 4;
			endcase
`elsif COVER_WITNESS_OLD_POST_RESET
			assume(reset_n == (witness_phase != 0 && witness_phase != 4));
			assume(avl_step && o_step && !waitrequest && !request_copy_retire);
			assume(vs_edge == (witness_phase == 1 && avl_reset_n &&
				return_drain && reference_words == 0));
			assume(schedule_read == (witness_phase == 2));
			assume(return_valid == (witness_phase == 5));
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
				2: if (read_start_event) witness_phase <= 3;
				3: if (issue_event) witness_phase <= 4;
				4: witness_phase <= 5;
			endcase
`elsif COVER_WITNESS_VS_ALIGN
			assume(reset_n == (witness_phase != 0));
			assume(avl_step && o_step && !waitrequest && !return_valid);
			assume(!schedule_read && !request_copy_retire);
			assume(vs_edge == (witness_phase == 1 && avl_reset_n &&
				return_drain && reference_words == 0));
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
			endcase
`elsif COVER_WITNESS_DRAIN_NO_VS
			assume(reset_n == (witness_phase != 0));
			assume(avl_step && o_step && !waitrequest && !return_valid);
			assume(!vs_edge && !schedule_read && !request_copy_retire);
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
			endcase
`elsif COVER_WITNESS_FIRST_COMPLETION
			assume(reset_n == (witness_phase != 0));
			assume(avl_step && o_step && !waitrequest && !request_copy_retire);
			assume(vs_edge == (witness_phase == 1 && avl_reset_n &&
				return_drain && reference_words == 0));
			assume(schedule_read == (witness_phase == 2));
			assume(return_valid ==
				(witness_phase == 4 && reference_words != 0));
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
				2: if (read_start_event) witness_phase <= 3;
				3: if (issue_event) witness_phase <= 4;
			endcase
`elsif COVER_WITNESS_ACTIVE_CREDIT_VS
			assume(reset_n == (witness_phase != 0));
			assume(avl_step && o_step && !waitrequest && !return_valid);
			assume(!request_copy_retire);
			assume(schedule_read == (witness_phase == 2));
			assume(vs_edge == ((witness_phase == 1 && avl_reset_n &&
				return_drain && reference_words == 0) ||
				(witness_phase == 4 && reference_words != 0)));
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
				2: if (read_start_event) witness_phase <= 3;
				3: if (issue_event) witness_phase <= 4;
			endcase
`elsif COVER_WITNESS_ISSUE_EMPTY_VS
			assume(reset_n == (witness_phase != 0));
			assume(avl_step && o_step && !return_valid);
			assume(waitrequest == (witness_phase == 3));
			assume(!request_copy_retire);
			assume(schedule_read == (witness_phase == 2));
			assume(vs_edge == ((witness_phase == 1 && avl_reset_n &&
				return_drain && reference_words == 0) || witness_phase == 4));
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
				2: if (read_start_event) witness_phase <= 3;
				3: if (read_active) witness_phase <= 4;
			endcase
`elsif COVER_WITNESS_FINAL_RETURN_VS_WAIT
			assume(reset_n == (witness_phase != 0 && witness_phase != 4));
			assume(avl_step && o_step && !waitrequest && !request_copy_retire);
			assume(schedule_read == (witness_phase == 2));
			assume(return_valid ==
				(witness_phase == 5 && reference_words != 0));
			assume(vs_edge == ((witness_phase == 1 && avl_reset_n &&
				return_drain && reference_words == 0) ||
				(witness_phase == 5 && reference_words == 1)));
			case (witness_phase)
				0: witness_phase <= 1;
				1: if (release_event) witness_phase <= 2;
				2: if (read_start_event) witness_phase <= 3;
				3: if (issue_event) witness_phase <= 4;
				4: witness_phase <= 5;
			endcase
`endif
			// Independent ordered DDR responder. A charged request creates exactly
			// BLEN return obligations, regardless of reset or waitrequest. The HPS
			// F2SDRAM path has nonzero response latency, so a beat must retire a
			// pre-edge obligation; issue+return remains legal for an older burst.
			if (return_event)
				assume(reference_words != 0);
			if (issue_event && !return_event)
				assert(reference_words <= BLEN);
			if (issue_event && return_event)
				assert(reference_words <= BLEN + 1);
			case ({issue_event, return_event})
				2'b10: reference_words <= reference_words + BLEN;
				2'b01: reference_words <= reference_words - 1'b1;
				2'b11: reference_words <= reference_words + BLEN - 1'b1;
				default: reference_words <= reference_words;
			endcase
			if (issue_event) begin
				assert(avl_step && read_active && !read_accepted);
				assert(!waitrequest);
			end
			if (read_assert_event)
				assert(read_pending != 0 && !read_active && !return_drain);

			assert(!accounting_invalid);

			assert(!queue_overflow);
			assert(readlev <= 2);
			assert(copylev <= 2);
			assert(read_pending <= 2);
			if (read_accepted)
				assert(!issue_event);
			assert(output_obligations <= {1'b0, readlev});
			if (!return_drain)
				assert(scheduler_obligations <= {2'b00, readlev});
			if (completion_seen)
				assert(unseen_completions != 0);

			if (!avl_reset_n || !o_reset_n) begin
				unseen_completions_state <= 0;
			end else begin
				case ({completion_event, completion_seen})
					2'b10: unseen_completions_state <= unseen_completions + 1'b1;
					2'b01: unseen_completions_state <= unseen_completions - 1'b1;
					default: unseen_completions_state <= unseen_completions;
				endcase
				assert(unseen_completions <= 2);
			end

			if (return_drain) begin
				assert(!write_event);
				assert(!completion_event);
			end
			if (align_event) begin
				assert(avl_step && avl_reset_n);
				assert(vs_edge || release_event);
				assert(reference_words == 0);
				assert(return_credits == 0 && return_phase == 0);
				visible_beat <= 0;
			end else if (write_event) begin
				assert(completion_event == (visible_beat == BLEN-1));
				visible_beat <= visible_beat + 1'b1;
				if (first_post_drain_active && completion_event) begin
					first_post_drain_active <= 1'b0;
					cover_first_post_drain_completion <= 1'b1;
				end
			end
			if (release_event) begin
				assert(return_drain);
				assert(release_pending);
				assert(align_event);
				assert(reference_words == 0);
				assert(return_credits == 0 && return_phase == 0);
				first_post_drain_active <= 1'b1;
				if (vs_edge) begin
					cover_vs_alignment_during_drain <= 1'b1;
				end else begin
					cover_drain_release_without_vs <= 1'b1;
				end
			end
			if (vs_edge && avl_step && avl_reset_n && reset_n &&
				reference_words != 0) begin
				assert(!align_event);
				cover_active_credit_vs <= 1'b1;
			end
			if (align_event && issue_event && reference_words == 0)
				cover_issue_empty_vs <= 1'b1;
			if (vs_edge && return_drain && return_event &&
				reference_words == 1) begin
				assert(!release_event && !align_event);
				cover_final_return_vs_wait <= 1'b1;
			end

			if (!reset_n && (reference_words != 0 || issue_event))
				saw_reset_obligation <= 1'b1;
			if (!reset_n && return_event && reference_words == 1)
				cover_final_old_beat_during_reset <= 1'b1;
			if (saw_reset_obligation && reset_n && return_drain && return_event)
				cover_old_beat_after_reset <= 1'b1;
			if (completion_event && request_toggle == ack_sync)
				cover_coincident_ack_completion <= 1'b1;

			if (!reset_n) begin
				stopped_completion_count <= 0;
				resumed_delivery_count <= 0;
				saw_two_stopped <= 1'b0;
			end else if (!saw_two_stopped) begin
				if (stopped_completion_count == 0) begin
					if (completion_event && !o_step)
						stopped_completion_count <= 1;
				end else if (o_step) begin
					stopped_completion_count <= 0;
				end else if (completion_event) begin
					stopped_completion_count <= 2;
					saw_two_stopped <= 1'b1;
				end
			end else if (completion_seen) begin
				if (resumed_delivery_count == 1) begin
					resumed_delivery_count <= 2;
					cover_two_stopped_delivered <= 1'b1;
				end else if (resumed_delivery_count == 0) begin
					resumed_delivery_count <= 1;
				end
			end
		end

		if (past_valid && avl_reset_n &&
			$past(proof_edge && align_event)) begin
			assert(write_phase == 2*BLEN-1);
		end
		if (past_valid && avl_reset_n &&
			$past(proof_edge && release_event)) begin
			assert(!return_drain);
			assert(write_phase == 2*BLEN-1);
		end
		if (past_valid && avl_reset_n &&
			$past(proof_edge && avl_step && avl_reset_n &&
			vs_edge && words_remaining != 0)) begin
			assert(!$past(align_event));
			if ($past(write_event))
				assert(write_phase == (($past(write_phase) + 1) % (2*BLEN)));
			else
				assert(write_phase == $past(write_phase));
		end
		if (past_valid && $past(proof_edge && return_drain && return_event &&
			!release_event && !vs_edge))
			assert(write_phase == $past(write_phase));

		cover(cover_two_stopped_delivered);
		cover(cover_coincident_ack_completion);
		cover(cover_final_old_beat_during_reset);
		cover(cover_old_beat_after_reset);
		cover(cover_vs_alignment_during_drain);
		cover(cover_drain_release_without_vs);
		cover(cover_first_post_drain_completion);
		cover(cover_active_credit_vs);
		cover(cover_issue_empty_vs);
		cover(cover_final_return_vs_wait);
	end
endmodule
