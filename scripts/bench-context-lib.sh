#!/usr/bin/env bash
# Shared benchmark run-context helpers.

bench_context_size_bytes() {
  local bin_path="$1"
  if [[ -f "$bin_path" ]]; then
    if stat -f%z "$bin_path" >/dev/null 2>&1; then
      stat -f%z "$bin_path"
    else
      stat -c%s "$bin_path"
    fi
  else
    printf '0\n'
  fi
}

bench_context_binary_fields() {
  local profile="$1" ui_scope="$2" features="$3" binary_path="$4" runtime_type="$5" deployment_state="${6:-verified}"
  local size scope restore_required
  size="$(bench_context_size_bytes "$binary_path")"
  scope="${runtime_type}-${ui_scope}-scope"
  restore_required="0"
  if [[ "$deployment_state" != "verified" ]]; then
    runtime_type="deployed-unknown"
    scope="deployed-unknown"
    restore_required="unknown"
  elif [[ "$runtime_type" == "profile" ]]; then
    scope="profile-${ui_scope}-scope"
    restore_required="1"
  elif [[ "$ui_scope" == "all" ]]; then
    scope="prod-all"
  else
    scope="${ui_scope}-scope"
  fi
  printf 'ui_scope=%s\tprofile=%s\tfeatures=%s\tbinary_scope=%s\tbinary_path=%s\tbinary_size_bytes=%s\truntime_type=%s\tdeployment_state=%s\tproduction_restore_required=%s' \
    "$ui_scope" "$profile" "$features" "$scope" "$binary_path" "$size" "$runtime_type" "$deployment_state" "$restore_required"
}
