# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

package require ::quartus::project
package require ::quartus::sta

project_open menu
create_timing_netlist
read_sdc
update_timing_netlist

report_timing \
	-setup \
	-npaths 25 \
	-nworst 1 \
	-detail full_path \
	-file output_files/menu.top-setup-paths.rpt
report_timing \
	-hold \
	-npaths 25 \
	-nworst 1 \
	-detail full_path \
	-file output_files/menu.top-hold-paths.rpt

delete_timing_netlist
project_close
