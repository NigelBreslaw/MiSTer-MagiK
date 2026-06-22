#!/usr/bin/env bash
# Run MiSTer screenshot media download benchmarks and append tool output rows.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-screenshot-download.tsv"

LABEL=""
SYSTEM="all"
VARIANTS="identity"
MANIFEST_URL="${MISTER_MEDIA_MANIFEST_URL:-https://assets.mistermagik.com/mister-magik/v1/manifest.json}"
SAVE_PREFERENCE=0
REPLACE_LABEL=0

usage() {
  cat <<'EOF'
Usage: scripts/profile-screenshot-download.sh LABEL --system ID [--variants identity,gzip,brotli] [--manifest-url URL] [--save-preference] [--replace-label]

Benchmarks screenshot pack download paths on the MiSTer. The timing rows include
network download, decompression, save/publish, checksum verification, and total
time. gzip and brotli variants use Cloudflare negotiated compression for the
same R2 object, not separate .gz/.br files.

Default manifest:
  https://assets.mistermagik.com/mister-magik/v1/manifest.json
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --system) SYSTEM="${2:?}"; shift 2 ;;
    --variant) VARIANTS="${2:?}"; shift 2 ;;
    --variants) VARIANTS="${2:?}"; shift 2 ;;
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
mkdir -p "$BENCH_DIR"
if [[ "$REPLACE_LABEL" -eq 1 && -f "$TSV" ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^screenshot_download_bench_tsv	${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

args=(
  media-bench-download
  --label "$LABEL"
  --system "$SYSTEM"
  --variants "$VARIANTS"
  --manifest-url "$MANIFEST_URL"
)
if [[ "$SAVE_PREFERENCE" -eq 1 ]]; then
  args+=(--save-preference)
fi

"$MISTER" "${args[@]}"
echo "appended to $TSV"
