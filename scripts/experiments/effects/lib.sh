#!/usr/bin/env bash
# Shared host-side helpers for experimental effect profiling scripts.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

EFFECT_PROFILE_TMP_FILES=()

effect_profile_setup() {
  local output_subdir="$1"
  local results_file="$2"

  HERE="$(experiment_repo_root)"
  MISTER="$HERE/scripts/mister"
  REMOTE="${EFFECT_REMOTE:-/media/fat/mister-magik/mister-magik-fb}"
  OUT_DIR="$HERE/build/$output_subdir"
  RESULTS="$HERE/history/toolchain-bench/$results_file"
}

effect_default_label() {
  local prefix="$1"
  printf '%s-%s\n' "$prefix" "$(date -u +%Y%m%dT%H%M%SZ)"
}

effect_validate_label() {
  local label="$1"
  if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
    echo "label must contain only letters, numbers, _, ., or -" >&2
    return 2
  fi
}

effect_validate_mode() {
  local mode="$1"
  local noun="$2"
  if [[ ! "$mode" =~ ^[A-Za-z0-9_,.-]+$ ]]; then
    echo "--mode must be a comma-separated $noun label list or mega" >&2
    return 2
  fi
}

effect_validate_positive_int() {
  local value="$1"
  local option="$2"
  if [[ ! "$value" =~ ^[0-9]+$ || "$value" -lt 1 ]]; then
    echo "$option must be a positive integer" >&2
    return 2
  fi
}

effect_validate_nonnegative_int() {
  local value="$1"
  local option="$2"
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    echo "$option must be an integer" >&2
    return 2
  fi
}

effect_validate_fb_format() {
  case "$1" in
    565) ;;
    *)
      echo "--fb-format must be 565; RGB888 UI support was removed" >&2
      return 2
      ;;
  esac
}

effect_validate_preview_format() {
  case "$1" in
    png|derived-png|raw-rgb|raw-rgb565|raw565|rgb565|565) ;;
    *)
      echo "--preview-format must be png, derived-png, raw-rgb, or raw-rgb565" >&2
      return 2
      ;;
  esac
}

effect_resolve_secs() {
  local secs="$1"
  local mode="$2"
  local segment_secs="$3"
  local effect_count="$4"

  if [[ -n "$secs" ]]; then
    printf '%s\n' "$secs"
    return
  fi
  if [[ "$mode" == "mega" || "$mode" == "all" || "$mode" == "demo" ]]; then
    printf '%s\n' $((segment_secs * effect_count))
  else
    IFS=',' read -r -a selected_modes <<<"$mode"
    printf '%s\n' $((segment_secs * ${#selected_modes[@]}))
  fi
}

effect_prepare_output_dirs() {
  mkdir -p "$OUT_DIR" "$(dirname "$RESULTS")"
}

effect_prepare_results_file() {
  local results="$1"
  local header="$2"

  mkdir -p "$(dirname "$results")"
  if [[ ! -f "$results" ]] || ! head -1 "$results" | grep -q $'^label\teffect'; then
    echo "$header" >"$results"
  fi
}

effect_replace_results_label() {
  local results="$1"
  local label="$2"
  local tmp_results

  tmp_results="$(mktemp)"
  awk -v label="$label" 'NR == 1 || ($0 != "" && substr($0, 1, length(label) + 1) != label "\t")' "$results" >"$tmp_results"
  mv "$tmp_results" "$results"
}

effect_deploy_and_preflight() {
  local deploy="$1"
  local context="$2"

  case "$deploy" in
    device) "$HERE/scripts/deploy-rust.sh" --device --experiments ;;
    skip) : ;;
    *) echo "unknown deploy mode: $deploy" >&2; return 2 ;;
  esac
  require_experiment_binary "$MISTER" "$REMOTE" "$context"
}

effect_profile_paths() {
  local label="$1"
  local slug="$2"

  remote_tsv="/tmp/${label}-${slug}.tsv"
  remote_log="/tmp/${label}-${slug}.log"
  local_tsv="$OUT_DIR/${label}-${slug}.tsv"
  local_log="$OUT_DIR/${label}-${slug}.log"
}

effect_temp_file() {
  local var_name="$1"
  local tmp

  tmp="$(mktemp)"
  EFFECT_PROFILE_TMP_FILES+=("$tmp")
  printf -v "$var_name" '%s' "$tmp"
}

effect_cleanup_temp_files() {
  if [[ "${#EFFECT_PROFILE_TMP_FILES[@]}" -gt 0 ]]; then
    rm -f "${EFFECT_PROFILE_TMP_FILES[@]}"
  fi
}
