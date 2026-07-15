#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Run the bench-tools preview-pack-bench command on the MiSTer and summarize decode timings.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-pack-decode"

usage() {
  cat <<'USAGE'
Usage: scripts/profile-preview-pack-decode.sh LABEL [options]

Options:
  --deploy-device          Build/deploy a bench-tools-capable binary first.
  --pack PATH              Arcade pack to decode on-device.
  --variant NAME           Variant label (default mmlz4b-lz4-fast).
  --codec NAME             Codec label for TSV output (default lz4-flex).
  --iterations N           Decode iterations (default 5).
  --order NAME             sequential | random | catalog-scroll (default random).
  --sample all|N           Entry sample (default all).
  --arcade-pack PATH       Pack-size row path for arcade (default --pack).
  --saturn-pack PATH       Pack-size row path for saturn.
  --neogeo-pack PATH       Pack-size row path for neogeo.
  --self-test              Run parser self-test only.
USAGE
}

label="${1:-}"
if [[ "${label:-}" == "--self-test" ]]; then
  label="SELFTEST"
  self_test=1
  shift
else
  self_test=0
  if [[ -z "$label" ]]; then
    usage >&2
    exit 2
  fi
  shift
fi

pack="/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b"
variant="mmlz4b-lz4-fast"
codec="lz4-flex"
iterations="5"
order="random"
sample="all"
arcade_pack=""
saturn_pack="/media/fat/mister-magik/assets/saturn-screenshots-320x320.mmlz4b"
neogeo_pack="/media/fat/mister-magik/assets/neogeo-screenshots-320x320.mmlz4b"
deploy_device=0

while (($#)); do
  case "$1" in
    --deploy-device) deploy_device=1; shift ;;
    --pack) pack="${2:?--pack needs a path}"; shift 2 ;;
    --variant) variant="${2:?--variant needs a value}"; shift 2 ;;
    --codec) codec="${2:?--codec needs a value}"; shift 2 ;;
    --iterations) iterations="${2:?--iterations needs a value}"; shift 2 ;;
    --order) order="${2:?--order needs a value}"; shift 2 ;;
    --sample) sample="${2:?--sample needs a value}"; shift 2 ;;
    --arcade-pack) arcade_pack="${2:?--arcade-pack needs a path}"; shift 2 ;;
    --saturn-pack) saturn_pack="${2:?--saturn-pack needs a path}"; shift 2 ;;
    --neogeo-pack) neogeo_pack="${2:?--neogeo-pack needs a path}"; shift 2 ;;
    --self-test) self_test=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

arcade_pack="${arcade_pack:-$pack}"

summarize() {
  local file="$1"
  python3 - "$file" <<'PY'
import csv, math, sys
path = sys.argv[1]
rows = []
packs = []
with open(path, newline="") as f:
    header = None
    for line in f:
        line = line.rstrip("\n")
        if line.startswith("pack_size_tsv\t"):
            packs.append(line)
        elif line.startswith("preview_pack_bench_tsv\tlabel\t"):
            header = line.split("\t")
        elif line.startswith("preview_pack_bench_tsv\t"):
            parts = line.split("\t")
            if not header:
                continue
            row = dict(zip(header, parts))
            if row.get("result") == "ok":
                rows.append({
                    "asset": row["asset_key"],
                    "encoded": int(row["encoded_bytes"]),
                    "decoded": int(row["decoded_bytes"]),
                    "decode": int(row["decode_us"]),
                    "decode_cpu": int(row.get("decode_cpu_us") or 0),
                    "parse": int(row["raw565_parse_us"]),
                    "parse_cpu": int(row.get("raw565_parse_cpu_us") or 0),
                    "total": int(row["total_us"]),
                })
print("pack sizes:")
for row in packs:
    print(row)
if not rows:
    print("preview_pack_summary_tsv\trows=0\tvalid=0")
    sys.exit(1)
def pct(values, p):
    values = sorted(values)
    idx = math.ceil((p / 100.0) * len(values)) - 1
    return values[max(0, min(idx, len(values) - 1))]
decode = [r["decode"] for r in rows]
decode_cpu = [r["decode_cpu"] for r in rows if r["decode_cpu"]]
parse = [r["parse"] for r in rows]
parse_cpu = [r["parse_cpu"] for r in rows if r["parse_cpu"]]
total = [r["total"] for r in rows]
encoded = [r["encoded"] for r in rows]
decoded = [r["decoded"] for r in rows]
summary = (
    "preview_pack_summary_tsv"
    f"\trows={len(rows)}"
    f"\tdecode_p50_us={pct(decode,50)}"
    f"\tdecode_p95_us={pct(decode,95)}"
    f"\tdecode_p99_us={pct(decode,99)}"
    f"\tdecode_max_us={max(decode)}"
)
if decode_cpu:
    summary += (
        f"\tdecode_cpu_p50_us={pct(decode_cpu,50)}"
        f"\tdecode_cpu_p95_us={pct(decode_cpu,95)}"
        f"\tdecode_cpu_p99_us={pct(decode_cpu,99)}"
        f"\tdecode_cpu_max_us={max(decode_cpu)}"
    )
summary += (
    f"\traw565_parse_p99_us={pct(parse,99)}"
)
if parse_cpu:
    summary += f"\traw565_parse_cpu_p99_us={pct(parse_cpu,99)}"
summary += (
    f"\ttotal_p99_us={pct(total,99)}"
    f"\tavg_encoded_bytes={sum(encoded)//len(encoded)}"
    f"\tavg_decoded_bytes={sum(decoded)//len(decoded)}"
    "\tvalid=1"
)
print(summary)
print("slowest_preview_pack_entries_tsv\tasset_key\tdecode_us\tencoded_bytes\tdecoded_bytes")
for row in sorted(rows, key=lambda r: r["decode"], reverse=True)[:20]:
    print(f"slowest_preview_pack_entries_tsv\t{row['asset']}\t{row['decode']}\t{row['encoded']}\t{row['decoded']}")
PY
}

if [[ "$self_test" == "1" ]]; then
  tmp="$(mktemp)"
  trap 'rm -f "$tmp"' EXIT
  cat >"$tmp" <<'EOF'
pack_size_tsv	variant=test	system=arcade	bytes=100	entries=2	raw_bytes=200	ratio=0.5
preview_pack_bench_tsv	label	variant	codec	iteration	ordinal	asset_key	offset	entry_flag	encoded_bytes	decoded_bytes	compression_ratio	width	height	load_source	index_lookup_us	read_us	decode_us	raw565_parse_us	total_us	decode_mb_s	total_mb_s	checksum32	result	error
preview_pack_bench_tsv	L	test	lz4	1	1	a	0	lz4_block	10	20	0.5	2	2	archive_mem	0	0	5	1	6	1	1	00000001	ok	
preview_pack_bench_tsv	L	test	lz4	1	2	b	0	lz4_block	30	40	0.75	2	2	archive_mem	0	0	15	1	16	1	1	00000002	ok	
EOF
  summarize "$tmp"
  echo "profile-preview-pack-decode self-test ok"
  exit 0
fi

if [[ "$deploy_device" == "1" ]]; then
  "$HERE/scripts/deploy-rust.sh" --device --bench-tools --ui-scope launcher
fi

mkdir -p "$OUT_DIR"
local_tsv="$OUT_DIR/${label}-preview-pack.tsv"
remote_tsv="/tmp/${label}-preview-pack.tsv"
remote_log="/tmp/${label}-preview-pack.log"

remote_cmd=(
  /media/fat/mister-magik/mister-magik-fb preview-pack-bench
  --label "$label"
  --variant "$variant"
  --codec "$codec"
  --pack "$pack"
  --iterations "$iterations"
  --order "$order"
  --warm full
  --cache decoded-off
  --sample "$sample"
  --pack-size "arcade=$arcade_pack"
  --pack-size "saturn=$saturn_pack"
  --pack-size "neogeo=$neogeo_pack"
)

printf -v quoted ' %q' "${remote_cmd[@]}"
if ! "$MISTER" run "rm -f '$remote_tsv' '$remote_log';${quoted} >'$remote_tsv' 2>'$remote_log'" >/dev/null; then
  "$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null || true
  "$MISTER" get "$remote_log" "$OUT_DIR/${label}-preview-pack.log" >/dev/null || true
  if grep -q "unknown command 'preview-pack-bench'" "$local_tsv" "$OUT_DIR/${label}-preview-pack.log" 2>/dev/null; then
    echo "ERROR: deployed mister-magik-fb does not expose preview-pack-bench." >&2
    echo "Build/deploy a bench-tools-capable binary first: scripts/deploy-rust.sh --device --bench-tools" >&2
    exit 3
  fi
  echo "preview pack decode profile failed; see $OUT_DIR/${label}-preview-pack.log" >&2
  exit 1
fi
"$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null
"$MISTER" get "$remote_log" "$OUT_DIR/${label}-preview-pack.log" >/dev/null || true
echo "wrote $local_tsv"
summarize "$local_tsv"
