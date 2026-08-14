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
		PROCEDURE produce IS
		BEGIN
			WAIT UNTIL falling_edge(source_clk);
			completion<='1';
			WAIT UNTIL falling_edge(source_clk);
			completion<='0';
			produced<=produced+1;
		END PROCEDURE;
	BEGIN
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
