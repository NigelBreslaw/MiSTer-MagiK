# Fixed-purpose read-only netlist analysis on a disposable copy.
package require ::quartus::project
package require ::quartus::sta
project_open menu
create_timing_netlist
read_sdc
update_timing_netlist
source /reference/mister/platform/fpga/menu-vblank-latch/report_causal_payload.tcl
delete_timing_netlist
project_close
