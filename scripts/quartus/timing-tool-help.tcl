# Read-only installed-tool capability reference; no project or timing netlist.
package require ::quartus::sta
foreach command {set_net_delay set_max_skew set_max_delay set_data_delay report_timing get_timing_paths get_path_info get_fanouts get_edge_info get_path get_available_operating_conditions get_operating_conditions_info set_operating_conditions} {
 puts "MAGIK_TOOL_HELP $command"
 if {[llength [info commands $command]]} {
  puts [$command -long_help]
 } else {
  puts "UNAVAILABLE $command"
 }
}
