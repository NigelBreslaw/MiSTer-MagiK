# Full combinational CDC payload delays, independent of clock exceptions.
# get_path -pairs_only returns the longest path for EVERY connected register
# pair. npaths=0 prevents truncation. All device operating corners are checked.
set payload_file [open output_files/menu.magik-causal-payload.rpt w]
puts $payload_file "MagiK causal payload delay bound 10.000 ns"
set payload_groups [list \
 [list selector {*|select_first} {*|snapshot*}] \
 [list bank {*|snapshot*} {io_dout_sys*}] \
 [list output {*|output_hold*} {io_dout_sys*}] \
 [list output_crc {*|output_hold*} {*|crc_work*}] \
 [list crc {*|crc_work*} {io_dout_sys*}]]
set corner_index 0
foreach_in_collection op [get_available_operating_conditions] {
 set_operating_conditions $op
 update_timing_netlist
 puts $payload_file "CORNER $corner_index [get_operating_conditions_info $op -model] [get_operating_conditions_info $op -voltage] [get_operating_conditions_info $op -temperature]"
 foreach group $payload_groups {
  lassign $group label source_pattern destination_pattern
  # Restrict hierarchy selectors to this observer, not same-named legacy RTL.
  if {[string match {*|*} $source_pattern]} {set source_pattern "*mister_magik_scaler_causal_state:magik_scaler_causal_state[string range $source_pattern 1 end]"}
  if {[string match {*|*} $destination_pattern]} {set destination_pattern "*mister_magik_scaler_causal_state:magik_scaler_causal_state[string range $destination_pattern 1 end]"}
  set sources [get_registers -nowarn -no_duplicates $source_pattern]
  set destinations [get_registers -nowarn -no_duplicates $destination_pattern]
  set paths [get_path -from $sources -to $destinations -pairs_only -npaths 0]
  puts $payload_file "GROUP $corner_index $label [get_collection_size $sources] [get_collection_size $destinations] [get_collection_size $paths]"
  foreach_in_collection path $paths {
   set source_name [get_node_info -name [get_path_info -from $path]]
   set destination_name [get_node_info -name [get_path_info -to $path]]
   puts $payload_file "PATH $corner_index $label [get_path_info -arrival_time $path] $source_name $destination_name"
  }
 }
 incr corner_index
}
puts $payload_file "CORNERS $corner_index"
close $payload_file
