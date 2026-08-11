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
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|frozen*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|armed*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|mailbox_overrun*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|fault_trigger*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|snapshot_generation*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|route_epoch*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|route_flags*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|expected_base*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|expected_slot_end*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|first_address*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|last_address*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|accepted_bursts*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|expected_beats*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|returned_beats*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|outstanding*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|maximum_outstanding*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|maximum_waitrequest*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|oldest_request*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|no_read_frames*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|fault_flags*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|frame_count*}]]
# The pinned ascal drives vbuf_burstcount from constant BLEN=128, so Quartus
# folds the fault_burstcount payload word to a constant; no CDC source register
# exists or needs a bundled-data constraint.
set magik_diag_avalon_destination [magik_require_registers avalon_payload_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_verify_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_verify_sample*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|tx_crc*} \
	{*io_dout_sys*}]]

set magik_diag_output_source [magik_require_registers output_payload [list \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|frozen*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|armed*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|mailbox_overrun*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_trigger*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|snapshot_generation*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|route_epoch*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|active_sequence*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|snapshot_source_flags*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|snapshot_control_flags*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|reference_period*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|reference_lines*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|reference_pixels*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|reference_active_lines*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|reference_flags*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_period*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_lines*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_pixels*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_active_lines*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_flags*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|consecutive_black*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|consecutive_white*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|geometry_faults*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|frame_count*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|snapshot_heartbeat*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|control_changes*}]]
set magik_diag_output_destination [magik_require_registers output_payload_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_verify_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_verify_sample*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|tx_crc*} \
	{*io_dout_sys*}]]

set magik_diag_control_context [magik_require_registers control_context [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|expected_base*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|expected_slot_end*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|expected_route_epoch*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|expected_active_seq*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|expected_route_flags*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|generation*}]]
set magik_diag_avalon_context [magik_require_registers avalon_context [list \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|expected_base*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|expected_slot_end*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|route_epoch*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|route_flags*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|snapshot_generation*}]]
set magik_diag_output_context [magik_require_registers output_context [list \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|route_epoch*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|active_sequence*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|route_flags*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|snapshot_generation*}]]

set magik_diag_fault_source [magik_require_registers fault_trigger [list \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|fault_trigger*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_trigger*}]]
set magik_diag_fault_destination [magik_require_registers fault_trigger_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_trigger_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_trigger_sample*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_trigger_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_trigger_sample*}]]

set magik_diag_analyses [list \
	[list avalon_payload $magik_diag_avalon_source $magik_diag_avalon_destination] \
	[list output_payload $magik_diag_output_source $magik_diag_output_destination] \
	[list avalon_route $magik_diag_control_context $magik_diag_avalon_context] \
	[list output_route $magik_diag_control_context $magik_diag_output_context] \
	[list fault_trigger $magik_diag_fault_source $magik_diag_fault_destination]]

foreach analysis $magik_diag_analyses {
	lassign $analysis label source destination
	set_net_delay -max -from $source -to $destination 8.000
	set_max_skew -from $source -to $destination -exclude {ccpp} 2.000
	post_message -type info "MagiK diagnostics CDC analysis applied: $label"
}
