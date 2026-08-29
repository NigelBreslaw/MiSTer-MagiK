-- Copyright (C) 2026 Nigel Breslaw
-- SPDX-License-Identifier: GPL-3.0-or-later

LIBRARY ieee;
USE ieee.std_logic_1164.ALL;
USE ieee.numeric_std.ALL;
USE std.env.ALL;
USE work.mister_magik_scaler_completion_queue.ALL;

ENTITY tb_mister_magik_scaler_completion_queue IS
END ENTITY;

ARCHITECTURE test OF tb_mister_magik_scaler_completion_queue IS
	SIGNAL source_clk : std_logic:='0';
	SIGNAL destination_clk : std_logic:='0';
	SIGNAL destination_clock_enabled : std_logic:='1';
	SIGNAL reset_n : std_logic:='0';
	SIGNAL completion : std_logic:='0';
	SIGNAL request_toggle : std_logic:='0';
	SIGNAL completion_pending : std_logic:='0';
	SIGNAL request_meta : std_logic:='0';
	SIGNAL request_sync : std_logic:='0';
	SIGNAL completion_pulse : std_logic:='0';
	SIGNAL completion_ack_meta : std_logic:='0';
	SIGNAL completion_ack_sync : std_logic:='0';
	SIGNAL produced : natural:=0;
	SIGNAL consumed : natural:=0;
BEGIN
	source_clk<=NOT source_clk AFTER 5 ns;

	DestinationClock:PROCESS IS
	BEGIN
		WAIT FOR 3 ns;
		IF destination_clock_enabled='1' THEN
			destination_clk<=NOT destination_clk;
		ELSE
			destination_clk<='0';
		END IF;
	END PROCESS DestinationClock;

	Source:PROCESS(source_clk,reset_n) IS
		VARIABLE state_v : std_logic_vector(1 DOWNTO 0);
	BEGIN
		IF reset_n='0' THEN
			request_toggle<='0';
			completion_pending<='0';
			completion_ack_meta<='0';
			completion_ack_sync<='0';
		ELSIF rising_edge(source_clk) THEN
			completion_ack_meta<=request_sync;
			completion_ack_sync<=completion_ack_meta;
			ASSERT NOT completion_queue_overflow(
				request_toggle,completion_pending,completion_ack_sync,completion)
				REPORT "legal schedule overflowed" SEVERITY failure;
			state_v:=completion_queue_next(
				request_toggle,completion_pending,completion_ack_sync,completion);
			request_toggle<=state_v(1);
			completion_pending<=state_v(0);
		END IF;
	END PROCESS Source;

	Destination:PROCESS(destination_clk,reset_n) IS
	BEGIN
		IF reset_n='0' THEN
			request_meta<='0';
			request_sync<='0';
			completion_pulse<='0';
			consumed<=0;
		ELSIF rising_edge(destination_clk) THEN
			request_meta<=request_toggle;
			request_sync<=request_meta;
			completion_pulse<=request_meta XOR request_sync;
			IF completion_pulse='1' THEN
				consumed<=consumed+1;
			END IF;
		END IF;
	END PROCESS Destination;

	Stimulus:PROCESS IS
		VARIABLE state_v : std_logic_vector(1 DOWNTO 0);
		VARIABLE request_v,pending_v,ack_v,event_v : std_logic;
		VARIABLE credits_v,next_credits_v : natural;
		VARIABLE phase_v,next_phase_v : natural;
		VARIABLE remaining_v,next_remaining_v,expected_remaining_v : natural;
		VARIABLE issued_v,returned_v : boolean;
		VARIABLE drain_v : boolean;
		VARIABLE release_pending_v : boolean;
		VARIABLE visible_returns_v : natural;
		VARIABLE write_phase_v : natural;
		VARIABLE block_complete_v : boolean;
		VARIABLE vs_edge_v : boolean;
		VARIABLE read_asserted_v,read_accepted_v,waitrequest_v : std_logic;
		VARIABLE acceptance_count_v : natural;
		VARIABLE format_v : unsigned(2 DOWNTO 0);
		VARIABLE tail_last_v,tail_last1_v,tail_last2_v : std_logic;
		VARIABLE acpt_v,shift_count_v : natural;
		VARIABLE retired_v : boolean;
		PROCEDURE produce IS
		BEGIN
			WAIT UNTIL falling_edge(source_clk);
			completion<='1';
			WAIT UNTIL falling_edge(source_clk);
			completion<='0';
			produced<=produced+1;
		END PROCEDURE;
	BEGIN
		-- The exact production helper must preserve every legacy active case and
		-- add only the registered line-last tail.
		FOR hcarry_i IN 0 TO 1 LOOP
			FOR dshi_i IN 0 TO 3 LOOP
				FOR last_i IN 0 TO 1 LOOP
					ASSERT copy_shift_active(
						hcarry_i=1,dshi_i,to_unsigned(last_i,1)(0)) =
						(hcarry_i=1 OR dshi_i>0 OR last_i=1)
						REPORT "copy tail active predicate mismatch" SEVERITY failure;
				END LOOP;
			END LOOP;
		END LOOP;

		-- Normal non-last blocks still retire only on an aligned bank boundary.
		ASSERT copy_terminal_ready('1',true,true,'0','0')
			REPORT "normal non-last block no longer retires" SEVERITY failure;
		ASSERT NOT copy_terminal_ready('1',true,true,'1','0')
			REPORT "last block retired before its delayed line-last" SEVERITY failure;
		ASSERT copy_terminal_ready('1',true,false,'1','1')
			REPORT "delayed line-last no longer retires the last block" SEVERITY failure;

		-- Exhaust all starting pixel phases and supported output word formats.
		-- Once final hcarry registers o_last, two tail shifts drain last1/last2;
		-- every format reaches a next-word phase within 16 more shifts.
		FOR format_i IN 0 TO 3 LOOP
			CASE format_i IS
				WHEN 0 => format_v:="011"; -- 8bpp
				WHEN 1 => format_v:="100"; -- 16bpp
				WHEN 2 => format_v:="101"; -- 24bpp
				WHEN OTHERS => format_v:="110"; -- 32bpp
			END CASE;
			shift_count_v:=0;
			FOR phase_i IN 0 TO 15 LOOP
				IF copy_shift_onext(phase_i,format_v,128) THEN
					shift_count_v:=shift_count_v+1;
				END IF;
			END LOOP;
			ASSERT shift_count_v>0
				REPORT "supported format has no terminal word phase" SEVERITY failure;
			FOR start_phase IN 0 TO 15 LOOP
				tail_last_v:='1';
				tail_last1_v:='0';
				tail_last2_v:='0';
				acpt_v:=start_phase;
				retired_v:=false;
				FOR tail_step IN 0 TO 17 LOOP
					IF NOT retired_v THEN
						ASSERT copy_shift_active(false,0,tail_last_v)
							REPORT "line-last tail stopped" SEVERITY failure;
						IF copy_terminal_ready(
							'1',copy_shift_onext((acpt_v+1) MOD 16,format_v,128),
							false,'1',tail_last2_v) THEN
							retired_v:=true;
						ELSE
							tail_last2_v:=tail_last1_v;
							tail_last1_v:=tail_last_v;
							acpt_v:=(acpt_v+1) MOD 16;
						END IF;
					END IF;
				END LOOP;
				ASSERT retired_v
					REPORT "line-last tail failed bounded retirement" SEVERITY failure;
			END LOOP;
		END LOOP;

		-- Exhaust the exact production transition function truth table.
		FOR request_i IN 0 TO 1 LOOP
			FOR pending_i IN 0 TO 1 LOOP
				FOR ack_i IN 0 TO 1 LOOP
					FOR event_i IN 0 TO 1 LOOP
						request_v:=to_unsigned(request_i,1)(0);
						pending_v:=to_unsigned(pending_i,1)(0);
						ack_v:=to_unsigned(ack_i,1)(0);
						event_v:=to_unsigned(event_i,1)(0);
						state_v:=completion_queue_next(
							request_v,pending_v,ack_v,event_v);
						IF completion_queue_overflow(
							request_v,pending_v,ack_v,event_v) THEN
							ASSERT request_v/=ack_v AND pending_v='1' AND event_v='1'
								REPORT "overflow predicate mismatch" SEVERITY failure;
						ELSIF request_v=ack_v AND pending_v='1' THEN
							ASSERT state_v=((NOT request_v) & event_v)
								REPORT "pending-forward transition mismatch" SEVERITY failure;
						ELSIF request_v=ack_v AND event_v='1' THEN
							ASSERT state_v=((NOT request_v) & pending_v)
								REPORT "idle completion transition mismatch" SEVERITY failure;
						ELSIF request_v/=ack_v AND event_v='1' THEN
							ASSERT state_v=(request_v & '1')
								REPORT "busy queue transition mismatch" SEVERITY failure;
						ELSE
							ASSERT state_v=(request_v & pending_v)
								REPORT "hold transition mismatch" SEVERITY failure;
						END IF;
					END LOOP;
				END LOOP;
			END LOOP;
		END LOOP;

		-- Exhaust every reachable radix-BLEN state and prove that the exact
		-- production transitions are equivalent to the old remaining-word
		-- accounting for idle, issue, return, and simultaneous events.
		FOR credits_i IN 0 TO 2 LOOP
			FOR phase_i IN 0 TO 127 LOOP
				IF credits_i>0 OR phase_i=0 THEN
					remaining_v:=return_words_remaining(credits_i,phase_i,128);
					FOR issued_i IN 0 TO 1 LOOP
						FOR returned_i IN 0 TO 1 LOOP
							issued_v:=issued_i=1;
							returned_v:=returned_i=1;
							IF NOT return_accounting_invalid(
								credits_i,phase_i,issued_v,returned_v,128,256) THEN
								next_credits_v:=return_credits_next(
									credits_i,phase_i,issued_v,returned_v,128);
								next_phase_v:=return_phase_next(
									phase_i,returned_v,128);
								next_remaining_v:=return_words_remaining(
									next_credits_v,next_phase_v,128);
								expected_remaining_v:=remaining_v;
								IF issued_v THEN
									expected_remaining_v:=expected_remaining_v+128;
								END IF;
								IF returned_v THEN
									expected_remaining_v:=expected_remaining_v-1;
								END IF;
								ASSERT next_credits_v<=2 AND next_phase_v<128
									REPORT "radix return accounting exceeded capacity"
									SEVERITY failure;
								ASSERT next_remaining_v=expected_remaining_v
									REPORT "radix return accounting lost equivalence"
									SEVERITY failure;
								ASSERT next_credits_v/=0 OR next_phase_v=0
									REPORT "empty return accounting retained a partial phase"
									SEVERITY failure;
							END IF;
						END LOOP;
					END LOOP;
				END IF;
			END LOOP;
		END LOOP;

		-- A final old return may coincide with a new issue at full capacity. The
		-- completed credit is replaced in the same transition without overflow.
		ASSERT NOT return_accounting_invalid(2,127,true,true,128,256)
			REPORT "simultaneous issue/final return was rejected" SEVERITY failure;
		ASSERT return_credits_next(2,127,true,true,128)=2 AND
			return_phase_next(127,true,128)=0
			REPORT "simultaneous issue/final return transition mismatch"
			SEVERITY failure;

		-- A wait-stalled request is charged only on its first actual acceptance.
		-- The retained guard suppresses repeated low-waitrequest reset cycles.
		read_asserted_v:='1';
		read_accepted_v:='0';
		waitrequest_v:='1';
		acceptance_count_v:=0;
		credits_v:=0;
		phase_v:=0;
		ASSERT NOT read_obligation_accept(
			read_asserted_v,read_accepted_v,waitrequest_v,true)
			REPORT "wait-stalled request was charged before acceptance" SEVERITY failure;
		waitrequest_v:='0';
		ASSERT read_obligation_accept(
			read_asserted_v,read_accepted_v,waitrequest_v,true)
			REPORT "reset assertion acceptance edge was not charged" SEVERITY failure;
		credits_v:=return_credits_next(credits_v,phase_v,true,false,128);
		read_accepted_v:='1';
		acceptance_count_v:=acceptance_count_v+1;
		FOR reset_cycle IN 0 TO 7 LOOP
			ASSERT NOT read_obligation_accept(
				read_asserted_v,read_accepted_v,waitrequest_v,true)
				REPORT "retained request was accepted more than once during reset"
				SEVERITY failure;
		END LOOP;
		ASSERT acceptance_count_v=1
			REPORT "reset-held request did not have exactly one acceptance"
			SEVERITY failure;
		ASSERT return_words_remaining(credits_v,phase_v,128)=128
			REPORT "accepted reset request did not retain exactly one obligation"
			SEVERITY failure;
		FOR beat IN 0 TO 127 LOOP
			next_credits_v:=return_credits_next(
				credits_v,phase_v,false,true,128);
			phase_v:=return_phase_next(phase_v,true,128);
			credits_v:=next_credits_v;
		END LOOP;
		ASSERT credits_v=0 AND phase_v=0
			REPORT "wait-low terminator completion did not drain one obligation"
			SEVERITY failure;

		-- If reset releases before a stalled request is ever accepted, normal
		-- scheduler execution cancels it and no retained credit is created.
		read_asserted_v:='1';
		read_accepted_v:='0';
		waitrequest_v:='1';
		credits_v:=0;
		phase_v:=0;
		FOR reset_cycle IN 0 TO 3 LOOP
			ASSERT NOT read_obligation_accept(
				read_asserted_v,read_accepted_v,waitrequest_v,true)
				REPORT "unaccepted stalled request created a credit" SEVERITY failure;
		END LOOP;
		-- Reset has synchronously released into sIDLE before waitrequest drops.
		-- The stale retained read is immediately ineligible and cannot be accepted.
		waitrequest_v:='0';
		ASSERT NOT read_obligation_accept(
			read_asserted_v,read_accepted_v,waitrequest_v,false)
			REPORT "reset-release waitrequest drop accepted an orphan request"
			SEVERITY failure;
		read_asserted_v:='0';
		ASSERT return_words_remaining(credits_v,phase_v,128)=0
			REPORT "release-before-accept cancellation retained an obligation"
			SEVERITY failure;

		-- Counterexample to the old reset assumption: half of an accepted burst
		-- may arrive during reset and half after release. Retained accounting
		-- keeps the post-reset drain barrier closed until all 128 beats retire.
		credits_v:=return_credits_next(0,0,true,false,128);
		phase_v:=0;
		drain_v:=true;
		visible_returns_v:=0;
		FOR beat IN 0 TO 63 LOOP
			next_credits_v:=return_credits_next(
				credits_v,phase_v,false,true,128);
			phase_v:=return_phase_next(phase_v,true,128);
			credits_v:=next_credits_v;
			IF NOT drain_v THEN
				visible_returns_v:=visible_returns_v+1;
			END IF;
		END LOOP;
		ASSERT credits_v=1 AND phase_v=64 AND
			return_words_remaining(credits_v,phase_v,128)=64 AND
			visible_returns_v=0
			REPORT "reset-time stale returns were not retained for drain" SEVERITY failure;
		FOR beat IN 0 TO 63 LOOP
			next_credits_v:=return_credits_next(
				credits_v,phase_v,false,true,128);
			phase_v:=return_phase_next(phase_v,true,128);
			credits_v:=next_credits_v;
			IF NOT drain_v THEN
				visible_returns_v:=visible_returns_v+1;
			END IF;
		END LOOP;
		ASSERT credits_v=0 AND phase_v=0 AND visible_returns_v=0
			REPORT "post-reset stale returns escaped the drain barrier" SEVERITY failure;
		-- Discarded old beats update only retained accounting. Once empty, a
		-- one-bit pending state separates the wide phase alignment from the
		-- accounting comparator before drain reopens on the following edge.
		write_phase_v:=37;
		release_pending_v:=false;
		FOR stale_beat IN 1 TO 128 LOOP
			ASSERT drain_v
				REPORT "discarded stale beat opened drain" SEVERITY failure;
		END LOOP;
		ASSERT write_phase_v=37
			REPORT "discarded stale beats changed the retained write phase"
			SEVERITY failure;
		vs_edge_v:=false;
		ASSERT drain_v AND return_drain_ready(0,0)
			REPORT "empty accounting was not eligible for drain release"
			SEVERITY failure;
		release_pending_v:=true;
		ASSERT drain_v AND release_pending_v AND write_phase_v=37
			REPORT "empty accounting did not schedule an isolated release"
			SEVERITY failure;
		write_phase_v:=255;
		drain_v:=false;
		release_pending_v:=false;
		ASSERT write_phase_v=255 AND NOT drain_v AND NOT release_pending_v
			REPORT "pending release did not align phase before opening drain"
			SEVERITY failure;

		-- A final old return and VS can coincide, but both decisions observe the
		-- old nonempty accounting. The next cycle schedules release and the one
		-- after that aligns phase and opens drain before a new request can start.
		credits_v:=1;
		phase_v:=127;
		write_phase_v:=37;
		drain_v:=true;
		release_pending_v:=false;
		vs_edge_v:=true;
		ASSERT NOT return_drain_ready(credits_v,phase_v)
			REPORT "VS released drain before the final old return" SEVERITY failure;
		ASSERT write_phase_v=37 AND drain_v AND NOT release_pending_v
			REPORT "active old credit allowed coincident VS to align or release"
			SEVERITY failure;
		next_credits_v:=return_credits_next(
			credits_v,phase_v,false,true,128);
		next_phase_v:=return_phase_next(phase_v,true,128);
		credits_v:=next_credits_v;
		phase_v:=next_phase_v;
		ASSERT credits_v=0 AND phase_v=0
			REPORT "final old return did not empty accounting" SEVERITY failure;
		vs_edge_v:=false;
		ASSERT drain_v AND return_drain_ready(credits_v,phase_v)
			REPORT "drained accounting was not eligible on the next scheduler edge"
			SEVERITY failure;
		release_pending_v:=true;
		ASSERT drain_v AND release_pending_v AND write_phase_v=37
			REPORT "drained accounting did not enter pending release"
			SEVERITY failure;
		write_phase_v:=255;
		drain_v:=false;
		release_pending_v:=false;
		ASSERT write_phase_v=255 AND NOT drain_v AND NOT release_pending_v
			REPORT "pending release did not align and open the drain"
			SEVERITY failure;
		ASSERT return_words_remaining(
			return_credits_next(credits_v,phase_v,true,false,128),
			return_phase_next(phase_v,false,128),128)=128
			REPORT "new epoch did not start from empty return accounting" SEVERITY failure;

		-- An issue coincident with an empty-accounting VS after release observes
		-- the pre-edge empty state, aligns phase, and charges the new burst.
		write_phase_v:=91;
		vs_edge_v:=true;
		ASSERT return_drain_ready(credits_v,phase_v)
			REPORT "empty issue/VS edge was not eligible to align" SEVERITY failure;
		write_phase_v:=255;
		next_credits_v:=return_credits_next(
			credits_v,phase_v,true,false,128);
		next_phase_v:=return_phase_next(phase_v,false,128);
		credits_v:=next_credits_v;
		phase_v:=next_phase_v;
		ASSERT credits_v=1 AND phase_v=0 AND write_phase_v=255
			REPORT "issue coincident empty VS did not start aligned" SEVERITY failure;

		-- The active burst straddles another VS. Because its retained credit is
		-- nonempty, that edge cannot move write phase; completion remains beat BLEN.
		FOR new_beat IN 1 TO 128 LOOP
			block_complete_v:=(write_phase_v MOD 128)=126;
			IF new_beat<128 THEN
				ASSERT NOT block_complete_v
					REPORT "first new burst completed before beat BLEN" SEVERITY failure;
			ELSE
				ASSERT block_complete_v
					REPORT "first new burst did not complete on beat BLEN" SEVERITY failure;
			END IF;
			IF new_beat=64 THEN
				vs_edge_v:=true;
				ASSERT NOT return_drain_ready(credits_v,phase_v)
					REPORT "active burst was empty at straddling VS" SEVERITY failure;
			ELSE
				vs_edge_v:=false;
			END IF;
			write_phase_v:=(write_phase_v+1) MOD 256;
			next_credits_v:=return_credits_next(
				credits_v,phase_v,false,true,128);
			phase_v:=return_phase_next(phase_v,true,128);
			credits_v:=next_credits_v;
			IF new_beat=64 THEN
				ASSERT write_phase_v=63
					REPORT "active burst VS changed the write phase" SEVERITY failure;
			END IF;
		END LOOP;
		ASSERT write_phase_v=127 AND credits_v=0 AND phase_v=0
			REPORT "first new burst ended at the wrong write phase" SEVERITY failure;

		WAIT FOR 30 ns;
		reset_n<='1';
		WAIT FOR 40 ns;
		destination_clock_enabled<='0';
		produce;
		FOR beat IN 0 TO 127 LOOP
			WAIT UNTIL rising_edge(source_clk);
		END LOOP;
		produce;
		ASSERT completion_pending='1'
			REPORT "second stopped-clock completion was not queued" SEVERITY failure;
		destination_clock_enabled<='1';
		FOR timeout IN 0 TO 99 LOOP
			EXIT WHEN consumed=produced;
			WAIT UNTIL rising_edge(source_clk);
		END LOOP;
		ASSERT consumed=2 REPORT "two completions were not conserved" SEVERITY failure;
		ASSERT completion_pending='0' REPORT "pending completion did not drain" SEVERITY failure;

		reset_n<='0';
		WAIT FOR 30 ns;
		reset_n<='1';
		WAIT FOR 50 ns;
		ASSERT request_toggle='0' AND completion_pending='0' AND completion_pulse='0'
			REPORT "reset created stale completion state" SEVERITY failure;
		REPORT "PASS: exact VHDL queue function conserves stopped-clock completions";
		stop;
		WAIT;
	END PROCESS Stimulus;
END ARCHITECTURE test;
