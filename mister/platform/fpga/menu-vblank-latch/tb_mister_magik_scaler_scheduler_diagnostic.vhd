-- Copyright (C) 2026 Nigel Breslaw
-- SPDX-License-Identifier: GPL-3.0-or-later

LIBRARY ieee;
USE ieee.std_logic_1164.ALL;
USE ieee.numeric_std.ALL;
USE std.env.ALL;
USE work.mister_magik_scaler_completion_queue.ALL;

ENTITY tb_mister_magik_scaler_scheduler_diagnostic IS
END ENTITY;

ARCHITECTURE test OF tb_mister_magik_scaler_scheduler_diagnostic IS
BEGIN
	PROCESS IS
		VARIABLE source_v : std_logic_vector(6 DOWNTO 0);
		VARIABLE first_v,second_v : std_logic_vector(14 DOWNTO 0);
		VARIABLE word_v : std_logic_vector(15 DOWNTO 0);
	BEGIN
		-- Every public bit is bound to the exact production packing function.
		-- Source bit 3 is encoded as drain-inactive; the public word restores
		-- the required drain-active meaning.
		source_v:="1010010";
		first_v:=scheduler_diagnostic_candidate('1','1','0',2,1,source_v,'0');
		ASSERT first_v(0)='1' REPORT "running bit mismatch" SEVERITY failure;
		ASSERT first_v(1)='1' REPORT "read activity bit mismatch" SEVERITY failure;
		ASSERT first_v(2)='0' REPORT "completion activity bit mismatch" SEVERITY failure;
		ASSERT first_v(4 DOWNTO 3)="10" REPORT "read level mismatch" SEVERITY failure;
		ASSERT first_v(6 DOWNTO 5)="01" REPORT "copy level mismatch" SEVERITY failure;
		ASSERT first_v(7)=source_v(6) REPORT "request toggle mismatch" SEVERITY failure;
		ASSERT first_v(8)=source_v(5) REPORT "pending bit mismatch" SEVERITY failure;
		ASSERT first_v(9)=source_v(4) REPORT "acknowledgement mismatch" SEVERITY failure;
		ASSERT first_v(10)='0' REPORT "destination seen mismatch" SEVERITY failure;
		ASSERT first_v(11)=NOT source_v(3) REPORT "return drain mismatch" SEVERITY failure;
		ASSERT first_v(13 DOWNTO 12)=source_v(2 DOWNTO 1)
			REPORT "return credits mismatch" SEVERITY failure;
		ASSERT first_v(14)=source_v(0) REPORT "return phase mismatch" SEVERITY failure;

		-- A first sample and any changing second sample remain invalid.
		word_v:=scheduler_diagnostic_word('0',(OTHERS =>'0'),first_v);
		ASSERT word_v=first_v & '0' REPORT "first sample became coherent" SEVERITY failure;
		second_v:=first_v;
		second_v(1):=NOT second_v(1);
		word_v:=scheduler_diagnostic_word('1',first_v & '0',second_v);
		ASSERT word_v=second_v & '0' REPORT "changing sample became coherent" SEVERITY failure;

		-- Two identical completed-frame samples publish exactly the state ABI,
		-- with coherence in bit zero.
		word_v:=scheduler_diagnostic_word('1',first_v & '0',first_v);
		ASSERT word_v=first_v & '1' REPORT "stable sample packing mismatch" SEVERITY failure;
		ASSERT word_v(0)='1' REPORT "coherence bit mismatch" SEVERITY failure;
		ASSERT word_v(5 DOWNTO 4)="10" REPORT "public read level mismatch" SEVERITY failure;
		ASSERT word_v(7 DOWNTO 6)="01" REPORT "public copy level mismatch" SEVERITY failure;

		-- Exhaust the legal level encodings used by healthy, queue-backlog, and
		-- read-two/copy-zero credit-stall classifications.
		FOR read_level IN 0 TO 2 LOOP
			FOR copy_level IN 0 TO 2 LOOP
				first_v:=scheduler_diagnostic_candidate(
					'1','0','0',read_level,copy_level,"0000000",'0');
				word_v:=scheduler_diagnostic_word('1',first_v & '0',first_v);
				ASSERT to_integer(unsigned(word_v(5 DOWNTO 4)))=read_level
					REPORT "read level roundtrip failed" SEVERITY failure;
				ASSERT to_integer(unsigned(word_v(7 DOWNTO 6)))=copy_level
					REPORT "copy level roundtrip failed" SEVERITY failure;
			END LOOP;
		END LOOP;

		REPORT "PASS: exact scaler scheduler diagnostic packing and coherence"
			SEVERITY note;
		stop;
		WAIT;
	END PROCESS;
END ARCHITECTURE;
