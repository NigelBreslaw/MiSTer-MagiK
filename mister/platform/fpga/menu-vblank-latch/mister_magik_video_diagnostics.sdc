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

# Seven passive state bits cross from clk_100m into the HDMI-domain coherence
# sampler. The two encoded credit bits and the phase reduction are accepted
# only after identical completed-frame samples, but every physical route into
# the first synchronizer bank remains explicitly bounded.
set magik_scaler_diag_source [get_registers -nowarn -no_duplicates {
	*ascal:ascal|avl_readdataack
	*ascal:ascal|avl_completion_pending
	*ascal:ascal|avl_completion_ack_sync
	*ascal:ascal|avl_return_drain
	*ascal:ascal|avl_return_credits[*]
	*ascal:ascal|avl_diag_return_phase_nonzero
}]
if {[get_collection_size $magik_scaler_diag_source] != 7} {
	post_message -type error "MagiK scaler diagnostic source collection mismatch"
	error "MagiK scaler diagnostic source collection mismatch"
}
set magik_scaler_diag_source_meta [magik_require_registers diagnostic_source_meta \
	{*ascal:ascal|magik_diag_source_meta[*]} 7]
set magik_scaler_diag_source_route [get_registers -nowarn -no_duplicates {
	*ascal:ascal|avl_readdataack
	*ascal:ascal|avl_completion_pending
	*ascal:ascal|avl_completion_ack_sync
	*ascal:ascal|avl_return_credits[*]
	*ascal:ascal|avl_diag_return_phase_nonzero
}]
set magik_scaler_diag_source_route_count \
	[get_collection_size $magik_scaler_diag_source_route]
if {$magik_scaler_diag_source_route_count != 6} {
	post_message -type error "MagiK scaler diagnostic routed source collection mismatch"
	error "MagiK scaler diagnostic routed source collection mismatch"
}
set_net_delay -max 10.0 \
	-from $magik_scaler_diag_source_route \
	-to $magik_scaler_diag_source_meta

# Keep the drain crossing separate. A kept observer-only combinational node
# prevents Quartus from absorbing this monotonic state into control logic, so
# the source register has an ordinary bounded data route to the first stage.
set magik_scaler_diag_drain [get_registers -nowarn \
	{*ascal:ascal|avl_return_drain*}]
if {[get_collection_size $magik_scaler_diag_drain] < 1} {
	post_message -type error "MagiK diagnostic drain routed source is missing"
	error "MagiK diagnostic drain routed source is missing"
}
set magik_scaler_diag_drain_meta [magik_require_registers diagnostic_drain_meta \
	{*ascal:ascal|magik_diag_source_meta[3]} 1]
set_net_delay -max 10.0 \
	-from $magik_scaler_diag_drain \
	-to $magik_scaler_diag_drain_meta

# The state word is a bundled-data crossing. It is registered and held stable
# before the generation toggle changes; the receiver synchronizes that toggle
# and waits one additional clk_sys edge before sampling the word.
set magik_scaler_diag_generation [magik_require_registers diagnostic_generation \
	{*ascal:ascal|magik_diag_generation_i} 1]
set magik_scaler_diag_generation_meta [magik_require_registers diagnostic_generation_meta \
	{*magik_scaler_scheduler_diagnostic|generation_meta} 1]
set_net_delay -max 10.0 \
	-from $magik_scaler_diag_generation \
	-to $magik_scaler_diag_generation_meta

set magik_scaler_diag_word [get_registers -nowarn \
	{*ascal:ascal|magik_diag_word[*]}]
if {[get_collection_size $magik_scaler_diag_word] < 16} {
	post_message -type error "MagiK diagnostic bundled routed source is incomplete"
	error "MagiK diagnostic bundled routed source is incomplete"
}
set magik_scaler_diag_capture [magik_require_registers diagnostic_capture \
	{*magik_scaler_scheduler_diagnostic|snapshot_state[*]} 16]
set_net_delay -max 10.0 \
	-from $magik_scaler_diag_word \
	-to $magik_scaler_diag_capture

post_message -type info "MagiK diagnostics CDC analysis applied: scaler_completion_request_ack scaler_scheduler_state"
