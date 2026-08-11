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

# Max-skew must terminate at the direct register data inputs. Applying it to
# whole registers also includes enable/equality/readout cones, which are not
# bundled-data paths and can span unrelated bits. Quartus may implement a
# register input as d, asdata, or sdata, so require at least one real data pin
# for every logical register family while accepting any of those legal forms.
proc magik_require_data_pins {label register_patterns} {
	set pins [get_pins -nowarn -no_duplicates __magik_no_such_pin__]
	foreach register_pattern $register_patterns {
		set matches [get_pins -nowarn -no_duplicates __magik_no_such_pin__]
		foreach suffix [list d asdata sdata] {
			set suffix_matches [get_pins -nowarn -no_duplicates "${register_pattern}|${suffix}"]
			set matches [add_to_collection $matches $suffix_matches]
		}
		if {[get_collection_size $matches] == 0} {
			post_message -type error "MagiK diagnostics CDC data-pin collection is empty: $label $register_pattern"
			error "MagiK diagnostics CDC data-pin collection is empty: $label $register_pattern"
		}
		set pins [add_to_collection $pins $matches]
	}
	return $pins
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
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|first_address*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|last_address*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|accepted_bursts*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|returned_beats*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|fault_flags*}]]
set magik_diag_avalon_destination [magik_require_registers avalon_payload_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_verify_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_verify_sample*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|tx_crc*} \
	{*io_dout_sys*}]]
set magik_diag_avalon_coherence_source [magik_require_registers avalon_payload_coherence [list \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|snapshot_generation*}]]
set magik_diag_avalon_coherence_pins [magik_require_data_pins avalon_payload_coherence_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_verify_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_verify_sample*}]]

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
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|reference_active_lines*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_period*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|fault_flags*} \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|geometry_faults*}]]
set magik_diag_output_destination [magik_require_registers output_payload_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_verify_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_verify_sample*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|tx_crc*} \
	{*io_dout_sys*}]]
set magik_diag_output_coherence_source [magik_require_registers output_payload_coherence [list \
	{*mister_magik_video_diagnostics_output:magik_video_diagnostics_output|snapshot_generation*}]]
set magik_diag_output_coherence_pins [magik_require_data_pins output_payload_coherence_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_verify_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_verify_sample*}]]

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
set magik_diag_avalon_context_pins [magik_require_data_pins avalon_context_destination [list \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|expected_base*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|expected_slot_end*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|route_epoch*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|route_flags*} \
	{*mister_magik_video_diagnostics_avalon:magik_video_diagnostics_avalon|snapshot_generation*}]]
set magik_diag_output_context_pins [magik_require_data_pins output_context_destination [list \
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
set magik_diag_fault_destination_pins [magik_require_data_pins fault_trigger_destination [list \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_trigger_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|avalon_trigger_sample*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_trigger_candidate*} \
	{*mister_magik_video_diagnostics_control:magik_video_diagnostics|output_trigger_sample*}]]

set magik_diag_analyses [list \
	[list avalon_payload $magik_diag_avalon_source $magik_diag_avalon_destination $magik_diag_avalon_coherence_source $magik_diag_avalon_coherence_pins] \
	[list output_payload $magik_diag_output_source $magik_diag_output_destination $magik_diag_output_coherence_source $magik_diag_output_coherence_pins] \
	[list avalon_route $magik_diag_control_context $magik_diag_avalon_context $magik_diag_control_context $magik_diag_avalon_context_pins] \
	[list output_route $magik_diag_control_context $magik_diag_output_context $magik_diag_control_context $magik_diag_output_context_pins] \
	[list fault_trigger $magik_diag_fault_source $magik_diag_fault_destination $magik_diag_fault_source $magik_diag_fault_destination_pins]]

foreach analysis $magik_diag_analyses {
	lassign $analysis label net_source net_destination skew_source skew_destination
	set_net_delay -max -from $net_source -to $net_destination 8.000
	set_max_skew -from $skew_source -to $skew_destination -exclude {ccpp} 2.000
	post_message -type info "MagiK diagnostics CDC analysis applied: $label"
}
