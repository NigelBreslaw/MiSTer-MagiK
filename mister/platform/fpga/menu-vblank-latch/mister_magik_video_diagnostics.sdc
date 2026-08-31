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

set magik_fetch_record_ready [magik_require_registers fetch_record_ready \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready} 1]
set magik_fetch_record_ready_meta [magik_require_registers fetch_record_ready_meta \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|record_ready_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_fetch_record_ready \
	-to $magik_fetch_record_ready_meta

set magik_scheduler_snapshot_request [magik_require_registers scheduler_snapshot_request \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_request_toggle} 1]
set magik_scheduler_snapshot_request_meta [magik_require_registers scheduler_snapshot_request_meta \
	{*mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|request_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_scheduler_snapshot_request \
	-to $magik_scheduler_snapshot_request_meta

set magik_scheduler_snapshot_response [magik_require_registers scheduler_snapshot_response \
	{*mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|response_handoff_bit} 1]
set magik_scheduler_snapshot_response_meta [magik_require_registers scheduler_snapshot_response_meta \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_response_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_scheduler_snapshot_response \
	-to $magik_scheduler_snapshot_response_meta

# semantic_evidence[8:0] is a closed-loop multi-cycle path: it is written before
# the response handoff and remains immutable until a later request. Six one-hot
# outcomes plus three sidebands capture bit-for-bit into a destination bank
# with no other data source. The two-stage response synchronizer supplies more
# than one destination period of settling for every payload bit.
set magik_scheduler_snapshot_data [magik_require_registers scheduler_snapshot_data \
	{*mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|semantic_evidence*} 9]
set magik_scheduler_snapshot_destination [magik_require_registers scheduler_snapshot_destination \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|scheduler_snapshot_capture*} 9]
if {[get_collection_size $magik_scheduler_snapshot_destination] != 9} {
	post_message -type error "MagiK scheduler snapshot destination collection mismatch"
	error "MagiK scheduler snapshot destination collection mismatch"
}
set_net_delay -max 10.0 \
	-from $magik_scheduler_snapshot_data \
	-to $magik_scheduler_snapshot_destination

post_message -type info "MagiK diagnostics CDC analysis applied: scaler_completion_request_ack scaler_copy_tail scaler_fetch_terminal_record scheduler_snapshot_request_response_data reset_observed"
