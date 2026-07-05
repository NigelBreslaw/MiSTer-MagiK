#!/usr/bin/env bash
# Measure the first selected Arcade screenshot with preview archive warmup disabled.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$HERE/build/preview-scroll-profiles"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/preview-selection-lib.sh"

secs="8"
label="first-preview-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="--skip-build"
self_test="0"
replace_label=()
selected_index="${MISTER_FIRST_PREVIEW_SELECTED_INDEX:-}"

usage() {
  cat <<'EOF'
Usage: scripts/profile-first-preview.sh [LABEL] [--secs N] [--skip-build|--deploy-device|--replace-label|--selected-index N|--self-test]

Runs the supervised launcher Arcade preview trace with
MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM=1, then summarizes the first selected
preview decode/apply timing from the device log.
EOF
}

positionals=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs)
      secs="${2:?--secs needs a value}"
      shift 2
      ;;
    --skip-build)
      deploy="--skip-build"
      shift
      ;;
    --deploy-device)
      deploy="--deploy-device"
      shift
      ;;
    --replace-label)
      replace_label=(--replace-label)
      shift
      ;;
    --selected-index)
      selected_index="${2:?--selected-index needs a value}"
      shift 2
      ;;
    --self-test)
      self_test="1"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --*)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
    *)
      positionals+=("$1")
      shift
      ;;
  esac
done

if [[ "${#positionals[@]}" -ge 1 ]]; then label="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -gt 1 ]]; then usage >&2; exit 2; fi
if [[ ! "$secs" =~ ^[0-9]+$ ]]; then echo "secs must be an integer" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ -n "$selected_index" && ! "$selected_index" =~ ^[0-9]+$ ]]; then echo "--selected-index must be an integer" >&2; exit 2; fi

summarize_first_preview_log() {
  local summary_label="$1" log_path="$2"
  awk -v label="$summary_label" '
  function field(name, fallback,    i, kv) {
    for (i = 1; i <= NF; i++) {
      split($i, kv, "=")
      if (kv[1] == name) return kv[2]
    }
    return fallback
  }
  $1 == "startup_timing" && $2 == "preview_selected_decoded" && decoded_seen == 0 {
    decoded_seen = 1
    decoded_queue_age_us = field("queue_age_us", field("age_us", "0"))
    decoded_load_source = field("load_source", "unknown")
    decoded_total_us = field("total_us", "0")
    decoded_read_us = field("read_us", "0")
    decoded_decode_us = field("decode_us", "0")
    decoded_encoded_bytes = field("encoded_bytes", "0")
  }
  $1 == "startup_timing" && $2 == "preview_selected_applied" && apply_seen == 0 {
    apply_seen = 1
    apply_age_us = field("age_us", "0")
    apply_load_source = field("load_source", "unknown")
  }
  /preview_trace decoded / && decoded_seen == 0 {
    priority = field("priority", "")
    if (priority == "Selected") {
      decoded_seen = 1
      decoded_queue_age_us = field("queue_age_us", "0")
      decoded_load_source = field("load_source", "unknown")
      decoded_total_us = field("total_us", "0")
      decoded_read_us = field("read_us", "0")
      decoded_decode_us = field("decode_us", "0")
      decoded_encoded_bytes = field("encoded_bytes", "0")
    }
  }
  /preview_trace apply / && apply_seen == 0 {
    selected = field("selected", "")
    if (selected == "true" || selected == "1") {
      apply_seen = 1
      apply_age_us = field("age_us", "0")
      apply_load_source = field("load_source", "unknown")
    }
  }
  END {
    printf "first_preview_tsv\tlabel=%s\tdecoded_seen=%d\tapply_seen=%d\tdecoded_queue_age_us=%s\tapply_age_us=%s\tdecoded_load_source=%s\tapply_load_source=%s\tdecoded_total_us=%s\tdecoded_read_us=%s\tdecoded_decode_us=%s\tdecoded_encoded_bytes=%s\n",
      label, decoded_seen + 0, apply_seen + 0, decoded_queue_age_us + 0,
      apply_age_us + 0, decoded_load_source, apply_load_source,
      decoded_total_us + 0, decoded_read_us + 0, decoded_decode_us + 0,
      decoded_encoded_bytes + 0
  }
  ' "$log_path"
}

run_self_test() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  local startup="$tmp/startup.log"
  cat >"$startup" <<'EOF'
startup_timing	preview_selected_decoded	123ms	system=arcade	title=1942	has_preview=1	asset_key=1942	generation=0	load_source=index_pread	total_us=900	read_us=700	decode_us=200	raw565_parse_us=50	age_us=12
startup_timing	preview_selected_applied	124ms	system=arcade	selected_index=0	title=1942	has_preview=1	asset_key=1942	generation=0	load_source=index_pread	total_us=900	read_us=700	decode_us=200	age_us=33
EOF
  summarize_first_preview_log selftest-startup "$startup" \
    | grep -q $'decoded_seen=1\tapply_seen=1' \
    || { echo "self-test startup readiness failed" >&2; return 1; }

  local prefetch="$tmp/prefetch.log"
  cat >"$prefetch" <<'EOF'
preview_trace decoded generation=2 priority=Prefetch cache_hit=0 load_source=index_pread format=raw-rgb565 total_us=300 read_us=100 decode_us=200 encoded_bytes=10 path=b.png
EOF
  summarize_first_preview_log selftest-prefetch "$prefetch" \
    | grep -q $'decoded_seen=0\tapply_seen=0' \
    || { echo "self-test prefetch exclusion failed" >&2; return 1; }

  local async_selected="$tmp/async-selected.log"
  cat >"$async_selected" <<'EOF'
preview_trace decoded generation=1 priority=Selected queue_age_us=7 cache_hit=0 load_source=index_pread format=raw-rgb565 total_us=1000 read_us=900 decode_us=100 encoded_bytes=10 path=a.png
preview_trace apply generation=1 priority=Selected selected=true age_us=44 load_source=index_pread format=raw-rgb565 total_us=1000 read_us=900 decode_us=100 encoded_bytes=10 path=a.png
EOF
  summarize_first_preview_log selftest-async "$async_selected" \
    | grep -q $'decoded_seen=1\tapply_seen=1' \
    || { echo "self-test async selected fallback failed" >&2; return 1; }

  local cache_apply="$tmp/cache-apply.log"
  cat >"$cache_apply" <<'EOF'
startup_timing	preview_selected_applied	1ms	system=arcade	selected_index=0	title=1942	has_preview=1	asset_key=1942	generation=0	load_source=decoded_cache	total_us=0	read_us=0	decode_us=0	age_us=0
EOF
  summarize_first_preview_log selftest-cache "$cache_apply" \
    | grep -q $'decoded_seen=0\tapply_seen=1' \
    || { echo "self-test decoded-cache apply failed" >&2; return 1; }

  preview_selection_self_test

  echo "profile-first-preview self-test ok"
}

if [[ "$self_test" == "1" ]]; then
  run_self_test
  exit 0
fi

mkdir -p "$OUT_DIR"
if [[ -z "$selected_index" ]]; then
  selected_index="$(preview_selection_index_for_system "$MISTER" arcade)" || {
    echo "no preview-bearing arcade row found in launcher_catalog" >&2
    exit 1
  }
fi
echo "==> first-preview selected_index=$selected_index system=arcade"
set +e
"$HERE/scripts/profile-preview-scroll.sh" \
  "$secs" \
  preview-idle \
  "$label" \
  "$deploy" \
  "${replace_label[@]}" \
  --start-system arcade \
  --selected-index "$selected_index" \
  --defer-start-system \
  --skip-preview-warm \
  --visual-captures 0
profile_status=$?
set -e
if [[ "$profile_status" -ne 0 && "$profile_status" -ne 12 ]]; then
  exit "$profile_status"
fi

log="$OUT_DIR/${label}-arcade.log"
if [[ ! -f "$log" ]]; then
  echo "missing preview log: $log" >&2
  exit 1
fi

summary="$(summarize_first_preview_log "$label" "$log")"
printf '%s\n' "$summary"
if [[ "$summary" != *$'\tapply_seen=1'* ]]; then
  echo "first selected preview was not applied; see $log" >&2
  exit 1
fi
