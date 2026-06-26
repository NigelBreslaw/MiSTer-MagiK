#!/usr/bin/env bash
# Run preview-index-refresh-bench on the MiSTer and summarize per-system timings.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-index-refresh"

usage() {
  cat <<'USAGE'
Usage: scripts/profile-preview-index-refresh.sh LABEL

Runs the diagnostics preview-index-refresh-bench command on-device. The command
updates library preview availability from installed screenshot pack .idx files
without rescanning the library or decoding screenshot payloads.
USAGE
}

label="${1:-}"
if [[ -z "$label" || "$label" == "-h" || "$label" == "--help" ]]; then
  usage
  [[ -z "$label" ]] && exit 2 || exit 0
fi
shift
if (($#)); then
  echo "unexpected argument: $1" >&2
  usage >&2
  exit 2
fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"
local_tsv="$OUT_DIR/${label}-preview-index-refresh.tsv"
local_log="$OUT_DIR/${label}-preview-index-refresh.log"
remote_tsv="/tmp/${label}-preview-index-refresh.tsv"
remote_log="/tmp/${label}-preview-index-refresh.log"

"$MISTER" run "rm -f '$remote_tsv' '$remote_log'; /media/fat/mister-magik/mister-magik-fb preview-index-refresh-bench '$label' >'$remote_tsv' 2>'$remote_log'" >/dev/null
"$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null
"$MISTER" get "$remote_log" "$local_log" >/dev/null || true

echo "wrote $local_tsv"
awk -F '\t' '
  $1 == "preview_index_refresh_tsv" && $2 != "label" {
    rows += 1
    systems[$3] = $11 ":" $12
    total_us += $11
    if ($12 == "ok") ok += 1
    else if ($12 ~ /^missing-/) missing += 1
    else errors += 1
  }
  END {
    printf "preview_index_refresh_summary_tsv\trows=%d\tok=%d\tmissing=%d\terrors=%d\ttotal_us=%d\n", rows, ok, missing, errors, total_us
    for (system in systems) {
      split(systems[system], parts, ":")
      printf "preview_index_refresh_system_tsv\t%s\ttotal_us=%s\tresult=%s\n", system, parts[1], parts[2]
    }
  }
' "$local_tsv"
