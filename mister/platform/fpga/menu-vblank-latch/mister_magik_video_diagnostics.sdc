# The physical HDMI FPLL lock is an asynchronous status indication. Exclude
# only its path into the forced first-stage register. The first-to-second-stage
# settling path remains timed and is included in metastability analysis.

proc magik_require_data_pin {label register_pattern} {
	set pins [get_pins -nowarn -no_duplicates __magik_no_such_pin__]
	foreach suffix [list d asdata sdata] {
		set matches [get_pins -nowarn -no_duplicates "${register_pattern}|${suffix}"]
		set pins [add_to_collection $pins $matches]
	}
	if {[get_collection_size $pins] != 1} {
		post_message -type error "MagiK HDMI evidence data-pin collection is not singular: $label"
		error "MagiK HDMI evidence data-pin collection is not singular: $label"
	}
	return $pins
}

set magik_hdmi_lock_meta_pin [magik_require_data_pin pll_lock_status \
	{*mister_magik_hdmi_lock_evidence:magik_hdmi_lock_evidence|control_pll_lock_meta}]
set_false_path -to $magik_hdmi_lock_meta_pin

# A registered two-bit Gray sequence carries scaler completion credits from
# clk_100m into clk_hdmi. Normal setup/hold is cut by the existing asynchronous
# clock groups, so explicitly bound both source-to-first-stage routes to one
# source period. The true source event spacing is a full 128-beat burst.
proc magik_require_registers {label register_pattern expected_count} {
	set registers [get_registers -nowarn -no_duplicates $register_pattern]
	if {[get_collection_size $registers] != $expected_count} {
		post_message -type error "MagiK scaler completion register collection mismatch: $label"
		error "MagiK scaler completion register collection mismatch: $label"
	}
	return $registers
}

set magik_scaler_completion_gray_source [magik_require_registers gray_source \
	{*ascal:ascal|avl_completion_gray_i[*]} 2]
set magik_scaler_completion_gray_meta [magik_require_registers gray_meta \
	{*ascal:ascal|o_completion_gray_meta[*]} 2]
set_net_delay -max 10.0 \
	-from $magik_scaler_completion_gray_source \
	-to $magik_scaler_completion_gray_meta
post_message -type info "MagiK diagnostics CDC analysis applied: scaler_completion_gray"
