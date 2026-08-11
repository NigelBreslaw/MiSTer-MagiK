# Passive bundled-data CDC constraints for the video diagnostic mailboxes.
# Each source field is immutable before acknowledgement. Every wildcard is
# checked independently so hierarchy drift cannot leave a partly constrained
# evidence bundle.

proc magik_require_registers {label patterns} {
	set registers [get_registers -nowarn -no_duplicates __magik_no_such_register__]
	foreach pattern $patterns {
		set matches [get_registers -nowarn -no_duplicates $pattern]
		if {[get_collection_size $matches] == 0} {
			post_message -type error "MagiK diagnostics CDC collection is empty: $label $pattern"
			error "MagiK diagnostics CDC collection is empty: $label $pattern"
		}
		set registers [add_to_collection $registers $matches]
	}
	return $registers
}

set magik_diag_avalon_source [magik_require_registers avalon_payload [list \
	{*|magik_video_diagnostics_avalon|frozen*} \
	{*|magik_video_diagnostics_avalon|armed*} \
	{*|magik_video_diagnostics_avalon|mailbox_overrun*} \
	{*|magik_video_diagnostics_avalon|fault_trigger*} \
	{*|magik_video_diagnostics_avalon|snapshot_generation*} \
	{*|magik_video_diagnostics_avalon|route_epoch*} \
	{*|magik_video_diagnostics_avalon|route_flags*} \
	{*|magik_video_diagnostics_avalon|expected_base*} \
	{*|magik_video_diagnostics_avalon|expected_slot_end*} \
	{*|magik_video_diagnostics_avalon|first_address*} \
	{*|magik_video_diagnostics_avalon|last_address*} \
	{*|magik_video_diagnostics_avalon|accepted_bursts*} \
	{*|magik_video_diagnostics_avalon|expected_beats*} \
	{*|magik_video_diagnostics_avalon|returned_beats*} \
	{*|magik_video_diagnostics_avalon|outstanding*} \
	{*|magik_video_diagnostics_avalon|maximum_outstanding*} \
	{*|magik_video_diagnostics_avalon|maximum_waitrequest*} \
	{*|magik_video_diagnostics_avalon|oldest_request*} \
	{*|magik_video_diagnostics_avalon|no_read_frames*} \
	{*|magik_video_diagnostics_avalon|fault_burstcount*} \
	{*|magik_video_diagnostics_avalon|fault_flags*} \
	{*|magik_video_diagnostics_avalon|frame_count*}]]
set magik_diag_avalon_destination [magik_require_registers avalon_payload_destination [list \
	{*|magik_video_diagnostics|avalon_verify_candidate*} \
	{*|magik_video_diagnostics|avalon_verify_sample*} \
	{*|magik_video_diagnostics|tx_crc*} \
	{*|io_dout_sys*}]]

set magik_diag_output_source [magik_require_registers output_payload [list \
	{*|magik_video_diagnostics_output|frozen*} \
	{*|magik_video_diagnostics_output|armed*} \
	{*|magik_video_diagnostics_output|mailbox_overrun*} \
	{*|magik_video_diagnostics_output|fault_trigger*} \
	{*|magik_video_diagnostics_output|snapshot_generation*} \
	{*|magik_video_diagnostics_output|route_epoch*} \
	{*|magik_video_diagnostics_output|active_sequence*} \
	{*|magik_video_diagnostics_output|snapshot_source_flags*} \
	{*|magik_video_diagnostics_output|snapshot_control_flags*} \
	{*|magik_video_diagnostics_output|reference_period*} \
	{*|magik_video_diagnostics_output|reference_lines*} \
	{*|magik_video_diagnostics_output|reference_pixels*} \
	{*|magik_video_diagnostics_output|reference_active_lines*} \
	{*|magik_video_diagnostics_output|reference_flags*} \
	{*|magik_video_diagnostics_output|fault_period*} \
	{*|magik_video_diagnostics_output|fault_lines*} \
	{*|magik_video_diagnostics_output|fault_pixels*} \
	{*|magik_video_diagnostics_output|fault_active_lines*} \
	{*|magik_video_diagnostics_output|fault_flags*} \
	{*|magik_video_diagnostics_output|consecutive_black*} \
	{*|magik_video_diagnostics_output|consecutive_white*} \
	{*|magik_video_diagnostics_output|geometry_faults*} \
	{*|magik_video_diagnostics_output|frame_count*} \
	{*|magik_video_diagnostics_output|snapshot_heartbeat*} \
	{*|magik_video_diagnostics_output|control_changes*}]]
set magik_diag_output_destination [magik_require_registers output_payload_destination [list \
	{*|magik_video_diagnostics|output_verify_candidate*} \
	{*|magik_video_diagnostics|output_verify_sample*} \
	{*|magik_video_diagnostics|tx_crc*} \
	{*|io_dout_sys*}]]

set magik_diag_control_context [magik_require_registers control_context [list \
	{*|magik_video_diagnostics|expected_base*} \
	{*|magik_video_diagnostics|expected_slot_end*} \
	{*|magik_video_diagnostics|expected_route_epoch*} \
	{*|magik_video_diagnostics|expected_active_seq*} \
	{*|magik_video_diagnostics|expected_route_flags*} \
	{*|magik_video_diagnostics|generation*}]]
set magik_diag_avalon_context [magik_require_registers avalon_context [list \
	{*|magik_video_diagnostics_avalon|expected_base*} \
	{*|magik_video_diagnostics_avalon|expected_slot_end*} \
	{*|magik_video_diagnostics_avalon|route_epoch*} \
	{*|magik_video_diagnostics_avalon|route_flags*} \
	{*|magik_video_diagnostics_avalon|snapshot_generation*}]]
set magik_diag_output_context [magik_require_registers output_context [list \
	{*|magik_video_diagnostics_output|route_epoch*} \
	{*|magik_video_diagnostics_output|active_sequence*} \
	{*|magik_video_diagnostics_output|route_flags*} \
	{*|magik_video_diagnostics_output|snapshot_generation*}]]

set magik_diag_fault_source [magik_require_registers fault_trigger [list \
	{*|magik_video_diagnostics_avalon|fault_trigger*} \
	{*|magik_video_diagnostics_output|fault_trigger*}]]
set magik_diag_fault_destination [magik_require_registers fault_trigger_destination [list \
	{*|magik_video_diagnostics|avalon_trigger_candidate*} \
	{*|magik_video_diagnostics|avalon_trigger_sample*} \
	{*|magik_video_diagnostics|output_trigger_candidate*} \
	{*|magik_video_diagnostics|output_trigger_sample*}]]

set magik_diag_analyses [list \
	[list avalon_payload $magik_diag_avalon_source $magik_diag_avalon_destination] \
	[list output_payload $magik_diag_output_source $magik_diag_output_destination] \
	[list avalon_route $magik_diag_control_context $magik_diag_avalon_context] \
	[list output_route $magik_diag_control_context $magik_diag_output_context] \
	[list fault_trigger $magik_diag_fault_source $magik_diag_fault_destination]]

foreach analysis $magik_diag_analyses {
	lassign $analysis label source destination
	set_net_delay -max -from $source -to $destination 8.000
	set_max_skew -from $source -to $destination 2.000
	post_message -type info "MagiK diagnostics CDC analysis applied: $label"
}
