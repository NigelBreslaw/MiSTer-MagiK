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

# Causal observer: four closed-loop mailbox crossings and observed reset.
# Payloads are held until the next explicit capture request. Response traverses
# two stages only after CRC completion; reads cannot see a bank while it rotates.
set magik_causal_capture_source [magik_require_registers causal_capture_source {*mister_magik_scaler_causal_state:magik_scaler_causal_state|capture_request} 1]
set magik_causal_capture_destination [magik_require_registers causal_capture_destination {*mister_magik_scaler_causal_state:magik_scaler_causal_state|capture_meta} 1]
set_net_delay -max 10.0 -from $magik_causal_capture_source -to $magik_causal_capture_destination
set magik_causal_response_source [magik_require_registers causal_response_source {*mister_magik_scaler_causal_state:magik_scaler_causal_state|response_toggle} 1]
set magik_causal_response_destination [magik_require_registers causal_response_destination {*mister_magik_scaler_causal_state:magik_scaler_causal_state|response_meta} 1]
set_net_delay -max 10.0 -from $magik_causal_response_source -to $magik_causal_response_destination
set magik_causal_output_request_source [magik_require_registers causal_output_request_source {*mister_magik_scaler_causal_state:magik_scaler_causal_state|output_request} 1]
set magik_causal_output_request_destination [magik_require_registers causal_output_request_destination {*mister_magik_scaler_causal_state:magik_scaler_causal_state|output_request_meta} 1]
set_net_delay -max 10.0 -from $magik_causal_output_request_source -to $magik_causal_output_request_destination
set magik_causal_output_response_source [magik_require_registers causal_output_response_source {*mister_magik_scaler_causal_state:magik_scaler_causal_state|output_response} 1]
set magik_causal_output_response_destination [magik_require_registers causal_output_response_destination {*mister_magik_scaler_causal_state:magik_scaler_causal_state|output_response_meta} 1]
set_net_delay -max 10.0 -from $magik_causal_output_response_source -to $magik_causal_output_response_destination
set magik_causal_reset_source [magik_require_registers causal_reset_source {*|reset_req} 1]
set magik_causal_reset_destination [magik_require_registers causal_reset_destination {*mister_magik_scaler_causal_state:magik_scaler_causal_state|reset_meta} 1]
set_net_delay -max 10.0 -from $magik_causal_reset_source -to $magik_causal_reset_destination
set magik_causal_bank [magik_require_registers causal_bank \
 {*mister_magik_scaler_causal_state:magik_scaler_causal_state|snapshot*} 32]
set magik_causal_output [magik_require_registers causal_output \
 {*mister_magik_scaler_causal_state:magik_scaler_causal_state|output_hold*} 16]
set magik_causal_selector [magik_require_registers causal_selector \
 {*mister_magik_scaler_causal_state:magik_scaler_causal_state|select_first} 1]
set magik_causal_crc [magik_require_registers causal_crc \
 {*mister_magik_scaler_causal_state:magik_scaler_causal_state|crc_work*} 16]
set magik_causal_uio [magik_require_registers causal_uio {*|io_dout_sys*} 16]
set_net_delay -max 10.0 -from $magik_causal_selector -to $magik_causal_bank
set_net_delay -max 10.0 -from $magik_causal_bank -to $magik_causal_uio
set_net_delay -max 10.0 -from $magik_causal_output -to $magik_causal_uio
set_net_delay -max 10.0 -from $magik_causal_output -to $magik_causal_crc
set_net_delay -max 10.0 -from $magik_causal_crc -to $magik_causal_uio
post_message -type info "MagiK diagnostics CDC analysis applied: scaler_completion_request_ack scaler_copy_tail causal_snapshot_request_response_data reset_observed"
