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

bench_context_sha256_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    printf 'missing\n'
    return 0
  fi
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{ print $1; exit }'
  else
    sha256sum "$path" | awk '{ print $1; exit }'
  fi
}

bench_context_parse_sha256() {
  awk '
    {
      for (i = 1; i <= NF; i++) {
        value = $i
        if (length(value) == 64 && value ~ /^[0-9A-Fa-f]+$/) {
          print tolower(value)
          exit
        }
      }
    }
  '
}

bench_context_shell_quote() {
  printf "'"
  printf '%s' "$1" | sed "s/'/'\"'\"'/g"
  printf "'"
}

bench_context_remote_sha256() {
  local mister="$1" remote_path="$2" output hash
  output="$("$mister" run "sha256sum $(bench_context_shell_quote "$remote_path")" 2>/dev/null)" || return 1
  hash="$(printf '%s\n' "$output" | bench_context_parse_sha256)"
  [[ -n "$hash" ]] || return 1
  printf '%s\n' "$hash"
}

bench_context_binary_identity_status() {
  local deployment_state="$1" local_sha="$2" deployed_sha="$3"
  if [[ "$deployment_state" != "verified" ]]; then
    printf 'unverified-deployment\n'
  elif [[ "$local_sha" == "missing" || -z "$local_sha" ]]; then
    printf 'missing-local\n'
  elif [[ "$deployed_sha" == "missing" || -z "$deployed_sha" ]]; then
    printf 'missing-deployed\n'
  elif [[ "$local_sha" != "$deployed_sha" ]]; then
    printf 'hash-mismatch\n'
  else
    printf 'verified\n'
  fi
}

bench_context_require_verified_identity() {
  [[ "$(bench_context_binary_identity_status "$1" "$2" "$3")" == "verified" ]]
}

bench_context_binary_features() {
  local binary_path="$1" feature_path="${1}.features" features
  if [[ ! -f "$feature_path" ]]; then
    printf 'missing\n'
    return 0
  fi
  features="$(tr -d '\r\n' <"$feature_path")"
  if [[ -z "$features" ]]; then
    printf 'missing\n'
  else
    printf '%s\n' "$features"
  fi
}

bench_context_build_receipt_path() {
  printf '%s.build-receipt.tsv\n' "$1"
}

bench_context_build_receipt_field() {
  local binary_path="$1" key="$2" receipt
  receipt="$(bench_context_build_receipt_path "$binary_path")"
  [[ -f "$receipt" ]] || return 1
  awk -F '\t' -v key="$key" '
    NR == 1 {
      for (i = 2; i <= NF; i++) {
        split($i, pair, "=")
        if (pair[1] == key) {
          sub("^[^=]*=", "", $i)
          print $i
          exit
        }
      }
    }
  ' "$receipt"
}

bench_context_build_receipt_status() {
  local binary_path="$1" expected_features="$2" expected_profile="${3:-}" expected_ui_scope="${4:-}"
  local local_sha receipt_sha receipt_features receipt_profile receipt_ui_scope receipt_commit receipt_dirty
  local_sha="$(bench_context_sha256_file "$binary_path")"
  receipt_sha="$(bench_context_build_receipt_field "$binary_path" binary_sha256 2>/dev/null || true)"
  receipt_features="$(bench_context_build_receipt_field "$binary_path" features 2>/dev/null || true)"
  receipt_profile="$(bench_context_build_receipt_field "$binary_path" profile 2>/dev/null || true)"
  receipt_ui_scope="$(bench_context_build_receipt_field "$binary_path" ui_scope 2>/dev/null || true)"
  receipt_commit="$(bench_context_build_receipt_field "$binary_path" source_commit 2>/dev/null || true)"
  receipt_dirty="$(bench_context_build_receipt_field "$binary_path" source_dirty 2>/dev/null || true)"
  if [[ -z "$receipt_sha" ]]; then
    printf 'missing\n'
  elif [[ "$local_sha" == "missing" || "$receipt_sha" != "$local_sha" ]]; then
    printf 'hash-mismatch\n'
  elif [[ "$receipt_features" != "$expected_features" ]]; then
    printf 'feature-mismatch\n'
  elif [[ -n "$expected_profile" && "$receipt_profile" != "$expected_profile" ]]; then
    printf 'profile-mismatch\n'
  elif [[ -n "$expected_ui_scope" && "$receipt_ui_scope" != "$expected_ui_scope" ]]; then
    printf 'ui-scope-mismatch\n'
  elif [[ -z "$receipt_commit" || ! "$receipt_dirty" =~ ^[01]$ ]]; then
    printf 'source-provenance-missing\n'
  else
    printf 'verified\n'
  fi
}

bench_context_write_build_receipt() {
  local binary_path="$1" repo="$2" profile="$3" features="$4" ui_scope="$5"
  local receipt tmp binary_sha source_fields
  receipt="$(bench_context_build_receipt_path "$binary_path")"
  tmp="$(mktemp "${receipt}.XXXXXX")"
  binary_sha="$(bench_context_sha256_file "$binary_path")"
  source_fields="$(bench_context_source_fields "$repo")"
  printf 'build_receipt_tsv\tbinary_sha256=%s\tprofile=%s\tfeatures=%s\tui_scope=%s\t%s\n' \
    "$binary_sha" "$profile" "$features" "$ui_scope" "$source_fields" >"$tmp"
  mv "$tmp" "$receipt"
}

bench_context_require_binary_contract() {
  local binary_path="$1" deployed_sha="$2" expected_features="$3" expected_profile="${4:-}" expected_ui_scope="${5:-}"
  local local_sha built_features receipt_status
  local_sha="$(bench_context_sha256_file "$binary_path")"
  built_features="$(bench_context_binary_features "$binary_path")"
  receipt_status="$(bench_context_build_receipt_status "$binary_path" "$expected_features" "$expected_profile" "$expected_ui_scope")"
  bench_context_require_verified_identity verified "$local_sha" "$deployed_sha" &&
    [[ "$built_features" == "$expected_features" ]] &&
    [[ "$receipt_status" == "verified" ]]
}

bench_context_source_dirty() {
  local repo="$1"
  if ! git -C "$repo" diff --quiet --; then
    printf '1\n'
    return 0
  fi
  if ! git -C "$repo" diff --cached --quiet --; then
    printf '1\n'
    return 0
  fi
  if [[ -n "$(git -C "$repo" ls-files --others --exclude-standard 2>/dev/null)" ]]; then
    printf '1\n'
  else
    printf '0\n'
  fi
}

bench_context_source_fields() {
  local repo="$1" commit commit_short dirty
  commit="$(git -C "$repo" rev-parse HEAD 2>/dev/null || printf 'unknown')"
  commit_short="$(git -C "$repo" rev-parse --short HEAD 2>/dev/null || printf 'unknown')"
  dirty="$(bench_context_source_dirty "$repo")"
  printf 'source_commit=%s\tsource_commit_short=%s\tsource_dirty=%s' \
    "$commit" "$commit_short" "$dirty"
}

bench_context_binary_fields() {
  local profile="$1" ui_scope="$2" features="$3" binary_path="$4" runtime_type="$5" deployment_state="${6:-verified}" deployed_sha="${7:-missing}"
  local size scope restore_required local_sha identity_status hash_match identity_valid built_features feature_match
  local build_receipt_status build_source_commit build_source_dirty build_profile build_ui_scope build_binary_sha
  size="$(bench_context_size_bytes "$binary_path")"
  local_sha="$(bench_context_sha256_file "$binary_path")"
  built_features="$(bench_context_binary_features "$binary_path")"
  build_receipt_status="$(bench_context_build_receipt_status "$binary_path" "$features" "$profile" "$ui_scope")"
  build_source_commit="$(bench_context_build_receipt_field "$binary_path" source_commit 2>/dev/null || printf 'missing')"
  build_source_dirty="$(bench_context_build_receipt_field "$binary_path" source_dirty 2>/dev/null || printf 'missing')"
  build_profile="$(bench_context_build_receipt_field "$binary_path" profile 2>/dev/null || printf 'missing')"
  build_ui_scope="$(bench_context_build_receipt_field "$binary_path" ui_scope 2>/dev/null || printf 'missing')"
  build_binary_sha="$(bench_context_build_receipt_field "$binary_path" binary_sha256 2>/dev/null || printf 'missing')"
  identity_status="$(bench_context_binary_identity_status "$deployment_state" "$local_sha" "$deployed_sha")"
  hash_match="unknown"
  identity_valid="0"
  feature_match="0"
  if [[ "$built_features" == "$features" ]]; then
    feature_match="1"
  fi
  if [[ "$local_sha" != "missing" && "$deployed_sha" != "missing" ]]; then
    if [[ "$local_sha" == "$deployed_sha" ]]; then
      hash_match="1"
    else
      hash_match="0"
    fi
  fi
  if [[ "$identity_status" == "verified" ]]; then
    identity_valid="1"
  fi
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
  printf 'ui_scope=%s\tprofile=%s\tfeatures=%s\tbuilt_features=%s\tbinary_feature_match=%s\tbuild_receipt_status=%s\tbuild_binary_sha256=%s\tbuild_profile=%s\tbuild_ui_scope=%s\tbuild_source_commit=%s\tbuild_source_dirty=%s\tbinary_scope=%s\tbinary_path=%s\tbinary_size_bytes=%s\tlocal_sha256=%s\tdeployed_sha256=%s\tbinary_hash_match=%s\tbinary_identity_valid=%s\tbinary_identity_status=%s\truntime_type=%s\tdeployment_state=%s\tproduction_restore_required=%s' \
    "$ui_scope" "$profile" "$features" "$built_features" "$feature_match" "$build_receipt_status" "$build_binary_sha" "$build_profile" "$build_ui_scope" "$build_source_commit" "$build_source_dirty" "$scope" "$binary_path" "$size" "$local_sha" "$deployed_sha" "$hash_match" "$identity_valid" "$identity_status" "$runtime_type" "$deployment_state" "$restore_required"
}
