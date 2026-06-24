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
REPLACE_LABEL=0
REMOTE_BIN="${MISTER_MAGIK_REMOTE_BIN:-/media/fat/mister-magik/mister-magik-fb}"

usage() {
  cat <<'EOF'
Usage: scripts/profile-screenshot-download.sh LABEL --system ID [--variant identity] [--iterations N] [--prime-cache] [--manifest-url URL] [--replace-label]

Benchmarks screenshot pack download paths inside the deployed MagiK binary on
the MiSTer. The timing rows include network download, decompression,
save-to-disk, checksum verification, and total time. MagiK benchmarks the raw
identity .mmlz4b object only; compressed objects are not decoded in the runtime.

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
case "$VARIANT" in
  identity|none|plain) ;;
  *) echo "MagiK only benchmarks the raw identity screenshot variant" >&2; exit 2 ;;
esac
mkdir -p "$BENCH_DIR"
if [[ "$REPLACE_LABEL" -eq 1 && -f "$TSV" ]]; then
  tmp="$(mktemp)"
  awk -v label="$LABEL" -F '\t' '
    NR == 1 { print; next }
    $1 == "screenshot_download_bench_tsv" && ($2 == label || index($2, label "-") == 1) { next }
    $1 == "stage_tsv" && index($0, "\tsuite_label=" label "\t") > 0 { next }
    { print }
  ' "$TSV" >"$tmp" || true
  mv "$tmp" "$TSV"
fi

shell_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

remote_cmd="$(shell_quote "$REMOTE_BIN") media-bench-download"
remote_cmd+=" --label $(shell_quote "$LABEL")"
remote_cmd+=" --system $(shell_quote "$SYSTEM")"
remote_cmd+=" --variant $(shell_quote "$VARIANT")"
remote_cmd+=" --iterations $(shell_quote "$ITERATIONS")"
remote_cmd+=" --manifest-url $(shell_quote "$MANIFEST_URL")"
if [[ "$PRIME_CACHE" -eq 1 ]]; then
  remote_cmd+=" --prime-cache"
fi

out="$("$MISTER" run "$remote_cmd")"
printf '%s\n' "$out"
if [[ ! -f "$TSV" ]]; then
  printf 'type\tlabel\tsystem\tvariant\tencoded_bytes\tdecoded_bytes\tdownload_ms\tdecompress_ms\tsave_ms\tverify_ms\ttotal_ms\twire_mbps\tdecoded_mbps\tetag\tcontent_encoding\tcf_cache_status\tresult\n' >"$TSV"
fi
printf '%s\n' "$out" | grep -E '^(screenshot_download_bench_tsv|stage_tsv)	' >>"$TSV" || true
if [[ "$SAVE_PREFERENCE" -eq 1 ]]; then
  echo "warning: --save-preference is ignored by the MagiK benchmark wrapper" >&2
fi
echo "appended to $TSV"
