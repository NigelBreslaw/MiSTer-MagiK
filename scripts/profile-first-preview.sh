#!/usr/bin/env bash
# Measure the first selected Arcade screenshot with preview archive warmup disabled.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$HERE/build/preview-scroll-profiles"

secs="8"
label="first-preview-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="--skip-build"

usage() {
  cat <<'EOF'
Usage: scripts/profile-first-preview.sh [LABEL] [--secs N] [--skip-build|--deploy-device]

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

mkdir -p "$OUT_DIR"
"$HERE/scripts/profile-preview-scroll.sh" \
  "$secs" \
  preview-idle \
  "$label" \
  "$deploy" \
  --skip-preview-warm \
  --visual-captures 0

log="$OUT_DIR/${label}-arcade.log"
if [[ ! -f "$log" ]]; then
  echo "missing preview log: $log" >&2
  exit 1
fi

awk -v label="$label" '
  function field(name, fallback,    i, kv) {
    for (i = 1; i <= NF; i++) {
      split($i, kv, "=")
      if (kv[1] == name) return kv[2]
    }
    return fallback
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
' "$log"
