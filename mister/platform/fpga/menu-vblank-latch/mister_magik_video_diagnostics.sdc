# The scaler completion request crosses from clk_100m into clk_hdmi and the
# stable destination observation returns as its acknowledgement. Normal
# setup/hold is cut by the existing asynchronous clock groups, so explicitly
# bound each source-to-first-stage route to one clk_100m period. The request
# changes only after a full 128-beat return; the acknowledgement changes only
# after the destination's second synchronizer stage observes that request.
proc magik_require_registers {label register_pattern expected_count} {
	set registers [get_registers -nowarn -no_duplicates $register_pattern]
	if {[get_collection_size $registers] != $expected_count} {
		post_message -type error "MagiK scaler completion register collection mismatch: $label"
		error "MagiK scaler completion register collection mismatch: $label"
	}
	return $registers
}

set magik_scaler_completion_request [magik_require_registers request_source \
	{*ascal:ascal|avl_readdataack} 1]
set magik_scaler_completion_request_meta [magik_require_registers request_meta \
	{*ascal:ascal|o_readdataack_sync} 1]
set_net_delay -max 10.0 \
	-from $magik_scaler_completion_request \
	-to $magik_scaler_completion_request_meta

set magik_scaler_completion_ack [magik_require_registers ack_source \
	{*ascal:ascal|o_readdataack_sync2} 1]
set magik_scaler_completion_ack_meta [magik_require_registers ack_meta \
	{*ascal:ascal|avl_completion_ack_meta} 1]
set magik_scaler_completion_ack_route [get_registers -nowarn -no_duplicates \
	{*ascal:ascal|o_readdataack_sync2*}]
set magik_scaler_completion_ack_route_count \
	[get_collection_size $magik_scaler_completion_ack_route]
if {$magik_scaler_completion_ack_route_count < 1 ||
	$magik_scaler_completion_ack_route_count > 2} {
	post_message -type error "MagiK scaler completion acknowledgement route mismatch"
	error "MagiK scaler completion acknowledgement route mismatch"
}
set_net_delay -max 10.0 \
	-from $magik_scaler_completion_ack_route \
	-to $magik_scaler_completion_ack_meta

# The Avalon activity bucket is registered and held stable before its
# generation changes. The o_clk receiver synchronizes that toggle and waits an
# additional edge before combining the stable bundle with output-domain frame
# activity.
set magik_scaler_avl_diag_generation [magik_require_registers avl_diagnostic_generation \
	{*ascal:ascal|avl_magik_generation} 1]
set magik_scaler_avl_diag_generation_meta [magik_require_registers avl_diagnostic_generation_meta \
	{*ascal:ascal|o_magik_generation_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_scaler_avl_diag_generation \
	-to $magik_scaler_avl_diag_generation_meta

set magik_scaler_avl_diag_word [magik_require_registers avl_diagnostic_word \
	{*ascal:ascal|avl_magik_bundle[*]} 16]
set magik_scaler_avl_diag_capture [magik_require_registers avl_diagnostic_capture \
	{*ascal:ascal|o_magik_diag_state[*]} 32]
set_net_delay -max 10.0 \
	-from $magik_scaler_avl_diag_word \
	-to $magik_scaler_avl_diag_capture

# The clk_hdmi responder merges the same-domain completed raw-frame flags into
# the stable ascal pipeline record, then repeats the toggle-plus-stable-bundle
# protocol into clk_sys. Existing asynchronous clock groups cut normal
# setup/hold analysis, so both routes are bounded explicitly.
set magik_scaler_diag_generation [magik_require_registers diagnostic_generation \
	{*magik_raw_scaler_diagnostic|source_generation} 1]
set magik_scaler_diag_generation_meta [magik_require_registers diagnostic_generation_meta \
	{*magik_raw_scaler_diagnostic|generation_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_scaler_diag_generation \
	-to $magik_scaler_diag_generation_meta

set magik_scaler_diag_word [magik_require_registers diagnostic_word \
	{*magik_raw_scaler_diagnostic|source_state[*]} 32]
set magik_scaler_diag_capture [magik_require_registers diagnostic_capture \
	{*magik_raw_scaler_diagnostic|snapshot_state[*]} 32]
set_net_delay -max 10.0 \
	-from $magik_scaler_diag_word \
	-to $magik_scaler_diag_capture

post_message -type info "MagiK diagnostics CDC analysis applied: scaler_completion_request_ack scaler_pipeline_state"
