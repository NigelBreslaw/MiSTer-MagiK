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
