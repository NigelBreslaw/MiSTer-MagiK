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
report_ucp \
	-file output_files/menu.unconstrained-paths.rpt

report_max_skew \
	-panel_name "MagiK Diagnostic CDC Skew" \
	-npaths 50 \
	-detail path_only \
	-file output_files/menu.magik-diagnostic-cdc-skew.rpt
report_net_delay \
	-panel_name "MagiK Diagnostic CDC Net Delay" \
	-nworst 100 \
	-file output_files/menu.magik-diagnostic-cdc-net-delay.rpt
report_metastability \
	-nchains 1000 \
	-file output_files/menu.magik-diagnostic-metastability.rpt
report_exceptions \
	-file output_files/menu.timing-exceptions.rpt

delete_timing_netlist
project_close
