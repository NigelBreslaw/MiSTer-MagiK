-- Copyright (C) 2026 Nigel Breslaw
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Narrow formal boundary for the production scaler completion scheduler.
--
-- The queue and return-accounting transitions below are the pure functions
-- compiled from the patched production ascal.vhd. The remaining registers are
-- the exact request synchronizer, registered completion pulse, reverse
-- acknowledgement synchronizer, and the read/copy credit truth tables from
-- ascal. Raster, pixel, RAM, and copy datapath state is deliberately replaced
-- by legal read-start and copy-retire choices.

LIBRARY ieee;
USE ieee.std_logic_1164.ALL;
USE ieee.numeric_std.ALL;
USE work.mister_magik_scaler_completion_queue.ALL;

ENTITY mister_magik_scaler_completion_formal_dut IS
	GENERIC (
		BLEN : positive:=128
	);
	PORT (
		clk                  : IN  std_logic;
		reset_n              : IN  std_logic;
		avl_step             : IN  std_logic;
		o_step               : IN  std_logic;
		waitrequest          : IN  std_logic;
		return_valid         : IN  std_logic;
		vs_edge              : IN  std_logic;
		schedule_read        : IN  std_logic;
		request_copy_retire  : IN  std_logic;

		avl_reset_n_o        : OUT std_logic;
		o_reset_n_o          : OUT std_logic;
		issue_event_o        : OUT std_logic;
		return_event_o       : OUT std_logic;
		write_event_o        : OUT std_logic;
		completion_event_o   : OUT std_logic;
		release_event_o      : OUT std_logic;
		read_start_event_o   : OUT std_logic;
		copy_retire_event_o  : OUT std_logic;
		completion_seen_o    : OUT std_logic;
		queue_overflow_o     : OUT std_logic;
		accounting_invalid_o : OUT std_logic;

		return_drain_o       : OUT std_logic;
		return_credits_o     : OUT natural RANGE 0 TO 2;
		return_phase_o       : OUT natural RANGE 0 TO BLEN-1;
		words_remaining_o    : OUT natural RANGE 0 TO 2*BLEN;
		write_phase_o        : OUT natural RANGE 0 TO 2*BLEN-1;
		request_toggle_o     : OUT std_logic;
		completion_pending_o : OUT std_logic;
		request_meta_o       : OUT std_logic;
		request_sync_o       : OUT std_logic;
		completion_pulse_o   : OUT std_logic;
		ack_meta_o           : OUT std_logic;
		ack_sync_o           : OUT std_logic;
		read_pending_o       : OUT natural RANGE 0 TO 2;
		read_active_o        : OUT std_logic;
		readlev_o            : OUT natural RANGE 0 TO 2;
		copylev_o            : OUT natural RANGE 0 TO 2
	);
END ENTITY;

ARCHITECTURE rtl OF mister_magik_scaler_completion_formal_dut IS
	SIGNAL avl_reset_n,o_reset_n : std_logic:='0';
	SIGNAL return_drain : std_logic:='1';
	SIGNAL return_credits : natural RANGE 0 TO 2:=0;
	SIGNAL return_phase : natural RANGE 0 TO BLEN-1:=0;
	SIGNAL write_phase : natural RANGE 0 TO 2*BLEN-1:=0;
	SIGNAL request_toggle,completion_pending : std_logic:='0';
	SIGNAL request_meta,request_sync,completion_pulse : std_logic:='0';
	SIGNAL ack_meta,ack_sync : std_logic:='0';
	SIGNAL read_pending : natural RANGE 0 TO 2:=0;
	SIGNAL read_active : std_logic:='0';
	SIGNAL readlev,copylev : natural RANGE 0 TO 2:=0;

	SIGNAL issue_event,return_event,write_event,completion_event : std_logic;
	SIGNAL release_event,read_start_event,copy_retire_event : std_logic;
	SIGNAL completion_seen,queue_overflow,accounting_invalid : std_logic;
BEGIN
	-- Reset assertion is common. Release is independently synchronized by the
	-- first formal step in each production clock domain.
	avl_reset_n_o<=avl_reset_n;
	o_reset_n_o<=o_reset_n;

	read_start_event<='1' WHEN reset_n='1' AND o_step='1' AND o_reset_n='1' AND
		schedule_read='1' AND readlev<2 ELSE '0';
	copy_retire_event<='1' WHEN reset_n='1' AND o_step='1' AND o_reset_n='1' AND
		request_copy_retire='1' AND readlev>0 AND copylev>0 ELSE '0';

	-- A read obligation is charged on the edge that first asserts the Avalon
	-- request. waitrequest may then hold read_active for an arbitrary duration;
	-- it cannot charge the request twice.
	issue_event<='1' WHEN reset_n='1' AND avl_step='1' AND avl_reset_n='1' AND
		return_drain='0' AND read_active='0' AND read_pending>0 ELSE '0';
	return_event<=avl_step AND return_valid;
	write_event<=return_event AND reset_n AND avl_reset_n AND NOT return_drain;
	completion_event<='1' WHEN write_event='1' AND
		(write_phase MOD BLEN)=BLEN-2 ELSE '0';
	release_event<='1' WHEN reset_n='1' AND avl_step='1' AND avl_reset_n='1' AND
		return_drain='1' AND vs_edge='1' AND
		return_drain_ready(return_credits,return_phase) ELSE '0';
	completion_seen<=reset_n AND o_step AND o_reset_n AND completion_pulse;
	queue_overflow<='1' WHEN completion_queue_overflow(
		request_toggle,completion_pending,ack_sync,completion_event) ELSE '0';
	accounting_invalid<='1' WHEN return_accounting_invalid(
		return_credits,return_phase,issue_event='1',return_event='1',BLEN,2*BLEN)
		ELSE '0';

	issue_event_o<=issue_event;
	return_event_o<=return_event;
	write_event_o<=write_event;
	completion_event_o<=completion_event;
	release_event_o<=release_event;
	read_start_event_o<=read_start_event;
	copy_retire_event_o<=copy_retire_event;
	completion_seen_o<=completion_seen;
	queue_overflow_o<=queue_overflow;
	accounting_invalid_o<=accounting_invalid;
	return_drain_o<=return_drain;
	return_credits_o<=return_credits;
	return_phase_o<=return_phase;
	words_remaining_o<=return_words_remaining(return_credits,return_phase,BLEN);
	write_phase_o<=write_phase;
	request_toggle_o<=request_toggle;
	completion_pending_o<=completion_pending;
	request_meta_o<=request_meta;
	request_sync_o<=request_sync;
	completion_pulse_o<=completion_pulse;
	ack_meta_o<=ack_meta;
	ack_sync_o<=ack_sync;
	read_pending_o<=read_pending;
	read_active_o<=read_active;
	readlev_o<=readlev;
	copylev_o<=copylev;

	Scheduler:PROCESS(clk) IS
		VARIABLE queue_state_v : std_logic_vector(1 DOWNTO 0);
		VARIABLE next_pending_v : integer RANGE 0 TO 2;
		VARIABLE next_readlev_v,next_copylev_v : integer RANGE 0 TO 2;
	BEGIN
		IF rising_edge(clk) THEN
			-- The retained accounting process has no core-reset branch.
			IF avl_step='1' THEN
				return_credits<=return_credits_next(
					return_credits,return_phase,issue_event='1',
					return_event='1',BLEN);
				return_phase<=return_phase_next(
					return_phase,return_event='1',BLEN);
			END IF;

			IF reset_n='0' THEN
				avl_reset_n<='0';
				o_reset_n<='0';
			ELSE
				IF avl_step='1' THEN
					avl_reset_n<='1';
				END IF;
				IF o_step='1' THEN
					o_reset_n<='1';
				END IF;
			END IF;

			-- Destination-domain scheduler and exact legacy credit truth tables.
			IF reset_n='0' OR o_reset_n='0' THEN
				request_meta<='0';
				request_sync<='0';
				completion_pulse<='0';
				readlev<=0;
				copylev<=0;
			ELSIF o_step='1' THEN
				request_meta<=request_toggle;
				request_sync<=request_meta;
				completion_pulse<=request_meta XOR request_sync;

				next_readlev_v:=readlev;
				IF read_start_event='1' THEN
					next_readlev_v:=next_readlev_v+1;
				END IF;
				IF copy_retire_event='1' THEN
					next_readlev_v:=next_readlev_v-1;
				END IF;
				readlev<=next_readlev_v;

				next_copylev_v:=copylev;
				IF completion_pulse='1' THEN
					next_copylev_v:=next_copylev_v+1;
				END IF;
				IF copy_retire_event='1' THEN
					next_copylev_v:=next_copylev_v-1;
				END IF;
				copylev<=next_copylev_v;
			END IF;

			-- read_pending bridges the abstracted legal read-start point to the
			-- production source-domain request assertion. Both sides are updated
			-- in this one formal process so arbitrary clock ordering is retained
			-- without a multi-driver proof model.
			IF reset_n='0' OR o_reset_n='0' THEN
				read_pending<=0;
			ELSE
				next_pending_v:=read_pending;
				IF read_start_event='1' THEN
					next_pending_v:=next_pending_v+1;
				END IF;
				IF issue_event='1' THEN
					next_pending_v:=next_pending_v-1;
				END IF;
				read_pending<=next_pending_v;
			END IF;

			-- Source-domain request, return drain, and Avalon request hold.
			IF reset_n='0' OR avl_reset_n='0' THEN
				return_drain<='1';
				request_toggle<='0';
				completion_pending<='0';
				ack_meta<='0';
				ack_sync<='0';
				read_active<='0';
			ELSIF avl_step='1' THEN
				ack_meta<=request_sync;
				ack_sync<=ack_meta;

				IF release_event='1' THEN
					return_drain<='0';
					write_phase<=2*BLEN-1;
				ELSIF write_event='1' THEN
					write_phase<=(write_phase+1) MOD (2*BLEN);
				END IF;

				queue_state_v:=completion_queue_next(
					request_toggle,completion_pending,ack_sync,completion_event);
				request_toggle<=queue_state_v(1);
				completion_pending<=queue_state_v(0);

				IF issue_event='1' THEN
					read_active<='1';
				ELSIF read_active='1' AND waitrequest='0' THEN
					read_active<='0';
				END IF;
			END IF;
		END IF;
	END PROCESS Scheduler;
END ARCHITECTURE;
