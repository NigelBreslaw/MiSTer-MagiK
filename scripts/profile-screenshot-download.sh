#!/usr/bin/env bash
# Run MiSTer screenshot media download benchmarks and append tool output rows.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-screenshot-download.tsv"

LABEL=""
SYSTEM="all"
VARIANT="identity"
MANIFEST_URL="${MISTER_MEDIA_MANIFEST_URL:-https://assets.mistermagik.com/mister-magik/v1/manifest.json}"
ITERATIONS=1
PRIME_CACHE=0
SAVE_PREFERENCE=0
MAX_SAVE_MS=""
SAVE_STRATEGY="staged"
REPLACE_LABEL=0
REMOTE_BIN="${MISTER_MAGIK_REMOTE_BIN:-/media/fat/mister-magik/mister-magik-fb}"

usage() {
  cat <<'EOF'
Usage: scripts/profile-screenshot-download.sh LABEL --system ID [--variant identity] [--iterations N] [--prime-cache] [--manifest-url URL] [--save-strategy staged|stream-fat] [--max-save-ms MS] [--replace-label]

Benchmarks screenshot pack download paths inside a deployed bench-tools MagiK
binary on the MiSTer. Build with `magik-gui/build-arm.sh --bench-tools` before
deploying this benchmark binary. The timing rows include network download,
decompression, save-to-disk, checksum verification, and total time. MagiK
benchmarks the raw identity .mmlz4b object only; compressed objects are not
decoded in the runtime.

Default manifest:
  https://assets.mistermagik.com/mister-magik/v1/manifest.json
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --system) SYSTEM="${2:?}"; shift 2 ;;
    --variant) VARIANT="${2:?}"; shift 2 ;;
    --variants) VARIANT="${2:?}"; shift 2 ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    --prime-cache) PRIME_CACHE=1; shift ;;
    --manifest-url) MANIFEST_URL="${2:?}"; shift 2 ;;
    --max-save-ms) MAX_SAVE_MS="${2:?}"; shift 2 ;;
    --save-strategy) SAVE_STRATEGY="${2:?}"; shift 2 ;;
    --save-preference) SAVE_PREFERENCE=1; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$LABEL" ]]; then
        echo "unexpected argument: $1" >&2
        usage >&2
        exit 2
      fi
      LABEL="$1"
      shift
      ;;
  esac
done

if [[ -z "$LABEL" ]]; then
  LABEL="screenshot-download-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "iterations must be a positive integer" >&2
  exit 2
fi
if [[ -n "$MAX_SAVE_MS" && ( ! "$MAX_SAVE_MS" =~ ^[0-9]+$ || "$MAX_SAVE_MS" -lt 1 ) ]]; then
  echo "max-save-ms must be a positive integer" >&2
  exit 2
fi
case "$VARIANT" in
  identity|none|plain) ;;
  *) echo "MagiK only benchmarks the raw identity screenshot variant" >&2; exit 2 ;;
esac
case "$SAVE_STRATEGY" in
  staged|stream-fat) ;;
  *) echo "save-strategy must be staged or stream-fat" >&2; exit 2 ;;
esac
mkdir -p "$BENCH_DIR"
if [[ "$REPLACE_LABEL" -eq 1 && -f "$TSV" ]]; then
  tmp="$(mktemp)"
  awk -v label="$LABEL" -F '\t' '
    NR == 1 { print; next }
    $1 == "screenshot_download_bench_tsv" && ($2 == label || index($2, label "-") == 1) { next }
    $1 == "stage_tsv" && index($0, "\tsuite_label=" label "\t") > 0 { next }
    $1 == "metric_tsv" && index($0, "\tsuite_label=" label "\t") > 0 { next }
    $1 == "validity_tsv" && index($0, "\tlabel=" label "\t") > 0 { next }
    { print }
  ' "$TSV" >"$tmp" || true
  mv "$tmp" "$TSV"
fi

shell_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

emit_metric_rows() {
  awk -F '\t' '
    function suite_label(label,   prefix, suffix) {
      prefix = label
      suffix = label
      if (match(label, /-[0-9][0-9]$/)) {
        prefix = substr(label, 1, length(label) - 3)
      }
      return prefix
    }
    function kv(name,   i, prefix) {
      prefix = name "="
      for (i = 1; i <= NF; i++) {
        if (index($i, prefix) == 1) {
          return substr($i, length(prefix) + 1)
        }
      }
      return ""
    }
    $1 == "screenshot_download_bench_tsv" && $2 != "label" {
      valid = ($17 == "bench-ok") ? 1 : 0
      suite = suite_label($2)
      print "metric_tsv\tlabel=" $2 "\tsuite_label=" suite "\tsystem=" $3 "\tmetric=screenshot_download_save_ms\tvalue=" $9 "\tunit=ms\tvalid=" valid
      print "metric_tsv\tlabel=" $2 "\tsuite_label=" suite "\tsystem=" $3 "\tmetric=screenshot_download_total_ms\tvalue=" $11 "\tunit=ms\tvalid=" valid
      print "metric_tsv\tlabel=" $2 "\tsuite_label=" suite "\tsystem=" $3 "\tmetric=screenshot_download_verify_ms\tvalue=" $10 "\tunit=ms\tvalid=" valid
      print "metric_tsv\tlabel=" $2 "\tsuite_label=" suite "\tsystem=" $3 "\tmetric=screenshot_download_wire_ms\tvalue=" $7 "\tunit=ms\tvalid=" valid
    }
    $1 == "stage_tsv" {
      label = kv("label")
      suite = kv("suite_label")
      system_id = kv("system")
      stage = kv("stage")
      ms = kv("ms")
      result = kv("result")
      if (label == "" || stage == "" || ms == "") {
        next
      }
      metric_stage = stage
      gsub(/[^A-Za-z0-9_.-]/, "_", metric_stage)
      valid = (result == "bench-ok") ? 1 : 0
      print "metric_tsv\tlabel=" label "\tsuite_label=" suite "\tsystem=" system_id "\tmetric=screenshot_download_stage_" metric_stage "_ms\tvalue=" ms "\tunit=ms\tvalid=" valid
    }
  '
}

emit_save_threshold_rows() {
  awk -F '\t' -v suite="$LABEL" -v limit="$MAX_SAVE_MS" '
    function add_system(system_id) {
      if (system_id == "") {
        return
      }
      if (systems == "") {
        systems = system_id
      } else if (index("," systems ",", "," system_id ",") == 0) {
        systems = systems "," system_id
      }
    }
    BEGIN {
      rows = 0
      max_save = 0
      valid = 1
      reason = "ok"
      systems = ""
    }
    $1 == "screenshot_download_bench_tsv" && $2 != "label" {
      rows += 1
      save_ms = $9 + 0
      add_system($3)
      if (save_ms > max_save) {
        max_save = save_ms
      }
      if ($17 != "bench-ok" && valid == 1) {
        valid = 0
        reason = "result_" $17
      }
      if (save_ms > limit && valid == 1) {
        valid = 0
        reason = "save_ms_gt_limit"
      }
    }
    END {
      if (rows == 0) {
        valid = 0
        reason = "missing_download_row"
      }
      print "metric_tsv\tlabel=" suite "\tsuite_label=" suite "\tsystem=" systems "\tmetric=screenshot_download_save_ms_max\tvalue=" max_save "\tunit=ms\tvalid=" valid
      print "validity_tsv\tlabel=" suite "\tvalid=" valid "\tinvalid_reason=" reason "\tdetail=max_save_ms=" max_save " limit_ms=" limit " rows=" rows " systems=" systems
      if (valid != 1) {
        exit 1
      }
    }
  '
}

remote_cmd="$(shell_quote "$REMOTE_BIN") media-bench-download"
remote_cmd+=" --label $(shell_quote "$LABEL")"
remote_cmd+=" --system $(shell_quote "$SYSTEM")"
remote_cmd+=" --variant $(shell_quote "$VARIANT")"
remote_cmd+=" --iterations $(shell_quote "$ITERATIONS")"
remote_cmd+=" --manifest-url $(shell_quote "$MANIFEST_URL")"
remote_cmd+=" --save-strategy $(shell_quote "$SAVE_STRATEGY")"
if [[ "$PRIME_CACHE" -eq 1 ]]; then
  remote_cmd+=" --prime-cache"
fi

out="$("$MISTER" run "$remote_cmd")"
printf '%s\n' "$out"
metric_rows="$(printf '%s\n' "$out" | emit_metric_rows)"
if [[ -n "$metric_rows" ]]; then
  printf '%s\n' "$metric_rows"
  out="${out}"$'\n'"${metric_rows}"
fi
threshold_status=0
if [[ -n "$MAX_SAVE_MS" ]]; then
  threshold_rows="$(printf '%s\n' "$out" | emit_save_threshold_rows)" || threshold_status=$?
  if [[ -n "$threshold_rows" ]]; then
    printf '%s\n' "$threshold_rows"
    out="${out}"$'\n'"${threshold_rows}"
  fi
fi
if [[ ! -f "$TSV" ]]; then
  printf 'type\tlabel\tsystem\tvariant\tencoded_bytes\tdecoded_bytes\tdownload_ms\tdecompress_ms\tsave_ms\tverify_ms\ttotal_ms\twire_mbps\tdecoded_mbps\tetag\tcontent_encoding\tcf_cache_status\tresult\n' >"$TSV"
fi
printf '%s\n' "$out" | grep -E '^(screenshot_download_bench_tsv|stage_tsv|metric_tsv|validity_tsv)	' >>"$TSV" || true
if [[ "$SAVE_PREFERENCE" -eq 1 ]]; then
  echo "warning: --save-preference is ignored by the MagiK benchmark wrapper" >&2
fi
echo "appended to $TSV"
if [[ "$threshold_status" -ne 0 ]]; then
  exit "$threshold_status"
fi
