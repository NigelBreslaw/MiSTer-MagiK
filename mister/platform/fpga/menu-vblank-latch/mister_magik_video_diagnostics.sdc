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

set magik_scheduler_snapshot_request [magik_require_registers scheduler_snapshot_request \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_request_toggle} 1]
set magik_scheduler_snapshot_request_meta [magik_require_registers scheduler_snapshot_request_meta \
	{*mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|request_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_scheduler_snapshot_request \
	-to $magik_scheduler_snapshot_request_meta

set magik_scheduler_snapshot_response [magik_require_registers scheduler_snapshot_response \
	{*mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|response_toggle} 1]
set magik_scheduler_snapshot_response_meta [magik_require_registers scheduler_snapshot_response_meta \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|snapshot_response_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_scheduler_snapshot_response \
	-to $magik_scheduler_snapshot_response_meta

# evidence_hold[15:1] is a closed-loop multi-cycle path: it is written before
# the response toggle and remains immutable until a later request. Bit zero is
# the completed-window marker and is deliberately forced high in every held
# response, so Quartus folds it to VCC rather than retaining a CDC register.
# The two-stage response synchronizer supplies more than one destination period
# of settling for every physical payload bit.
set magik_scheduler_snapshot_data [magik_require_registers scheduler_snapshot_data \
	{*mister_magik_scaler_scheduler_snapshot:scheduler_snapshot|evidence_hold*} 15]
set magik_scheduler_snapshot_destination [get_registers -nowarn -no_duplicates \
	{*mister_magik_scaler_fetch_liveness_state:magik_scaler_fetch_liveness_state|frozen_*}]
if {[get_collection_size $magik_scheduler_snapshot_destination] != 16} {
	post_message -type error "MagiK scheduler snapshot destination collection mismatch"
	error "MagiK scheduler snapshot destination collection mismatch"
}
set_net_delay -max 10.0 \
	-from $magik_scheduler_snapshot_data \
	-to $magik_scheduler_snapshot_destination

post_message -type info "MagiK diagnostics CDC analysis applied: scaler_completion_request_ack scaler_copy_tail scaler_fetch_liveness_publication_request_ack scheduler_snapshot_request_response_data reset_observed"
