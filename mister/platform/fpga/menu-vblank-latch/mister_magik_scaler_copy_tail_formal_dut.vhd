-- Copyright (C) 2026 Nigel Breslaw
-- SPDX-License-Identifier: GPL-3.0-or-later

LIBRARY ieee;
USE ieee.std_logic_1164.ALL;
USE ieee.numeric_std.ALL;
USE work.mister_magik_scaler_completion_queue.ALL;

ENTITY mister_magik_scaler_copy_tail_formal_dut IS
	PORT (
		clk                    : IN  std_logic;
		reset_n                : IN  std_logic;
		start                  : IN  std_logic;
		format                 : IN  unsigned(2 DOWNTO 0);
		phase                  : IN  natural RANGE 0 TO 15;
		active_o               : OUT std_logic;
		shift_o                : OUT std_logic;
		pixel_valid_o          : OUT std_logic;
		terminal_o             : OUT std_logic;
		done_o                 : OUT std_logic;
		age_o                  : OUT natural RANGE 0 TO 31;
		nonlast_terminal_o     : OUT std_logic;
		early_last_terminal_o  : OUT std_logic
	);
END ENTITY;

ARCHITECTURE rtl OF mister_magik_scaler_copy_tail_formal_dut IS
	SIGNAL active,last1,last2,done : std_logic:='0';
	SIGNAL acpt : natural RANGE 0 TO 15:=0;
	SIGNAL age : natural RANGE 0 TO 31:=0;
	SIGNAL active_format : unsigned(2 DOWNTO 0):="011";
	SIGNAL terminal : std_logic;
BEGIN
	active_o<=active;
	shift_o<='1' WHEN active='1' AND copy_shift_active(false,0,'1') ELSE '0';
	pixel_valid_o<='0';
	terminal_o<=terminal;
	done_o<=done;
	age_o<=age;
	nonlast_terminal_o<='1' WHEN
		copy_terminal_ready('1',true,true,'0','0') ELSE '0';
	early_last_terminal_o<='1' WHEN
		copy_terminal_ready('1',true,true,'1','0') ELSE '0';
	terminal<='1' WHEN active='1' AND copy_terminal_ready(
		'1',copy_shift_onext((acpt+1) MOD 16,active_format,128),
		false,'1',last2) ELSE '0';

	-- Starts on the first edge after final hcarry registered o_last. Tail edges
	-- advance only the exact production phase and delayed-last state; production
	-- forces pixel validity low on these same edges.
	CopyTail:PROCESS(clk,reset_n) IS
	BEGIN
		IF reset_n='0' THEN
			active<='0';
			last1<='0';
			last2<='0';
			acpt<=0;
			age<=0;
			active_format<="011";
			done<='0';
		ELSIF rising_edge(clk) THEN
			done<='0';
			IF active='1' THEN
				IF terminal='1' THEN
					active<='0';
					done<='1';
				ELSE
					last1<='1';
					last2<=last1;
					acpt<=(acpt+1) MOD 16;
					age<=age+1;
				END IF;
			ELSIF start='1' THEN
				active<='1';
				last1<='0';
				last2<='0';
				acpt<=phase;
				age<=0;
				active_format<=format;
			END IF;
		END IF;
	END PROCESS CopyTail;
END ARCHITECTURE;
