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
