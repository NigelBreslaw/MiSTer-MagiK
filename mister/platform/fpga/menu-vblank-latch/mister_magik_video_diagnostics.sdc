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

set magik_fetch_publication_generation [magik_require_registers fetch_publication_generation \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|publication_generation} 1]
set magik_fetch_publication_generation_meta [magik_require_registers fetch_publication_generation_meta \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|generation_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_fetch_publication_generation \
	-to $magik_fetch_publication_generation_meta

set magik_fetch_publication_ack [magik_require_registers fetch_publication_ack \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|acknowledged_generation} 1]
set magik_fetch_publication_ack_meta [magik_require_registers fetch_publication_ack_meta \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|acknowledge_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_fetch_publication_ack \
	-to $magik_fetch_publication_ack_meta

set magik_fetch_reset_req [magik_require_registers fetch_reset_source \
	{*reset_req} 1]
set magik_fetch_reset_meta [magik_require_registers fetch_reset_meta \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|reset_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_fetch_reset_req \
	-to $magik_fetch_reset_meta

post_message -type info "MagiK diagnostics CDC analysis applied: scaler_completion_request_ack scaler_copy_tail scaler_fetch_liveness_publication_request_ack_reset"
