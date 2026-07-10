#!/usr/bin/env bash
# Build and run the standalone RGB565 decimator microbenchmark on MiSTer.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT/build/rgb565-decimator-bench"
BIN="$OUT_DIR/rgb565-decimator-bench"
REMOTE_BIN="/tmp/rgb565-decimator-bench"
IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"

usage() {
  cat <<'USAGE'
Usage: scripts/profile-rgb565-decimator.sh LABEL [--samples N] [--runs N] [--cpu N] [--skip-build]

Builds a standalone C benchmark for the Cortex-A9 and runs repeated production
scalar RGB565 2x decimator measurements on the MiSTer. No Slint/Rust application
startup is involved. Raw TSV evidence is written under build/.
USAGE
}

label="${1:-}"
if [[ -z "$label" || "$label" == "-h" || "$label" == "--help" ]]; then
  usage
  [[ -z "$label" ]] && exit 2 || exit 0
fi
shift

samples=200
runs=5
cpu=0
skip_build=0
while (($#)); do
  case "$1" in
    --samples) samples="${2:?--samples needs N}"; shift 2 ;;
    --runs) runs="${2:?--runs needs N}"; shift 2 ;;
    --cpu) cpu="${2:?--cpu needs N}"; shift 2 ;;
    --skip-build) skip_build=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "LABEL must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
for value_name in samples runs; do
  value="${!value_name}"
  if [[ ! "$value" =~ ^[0-9]+$ || "$value" -lt 1 ]]; then
    echo "--$value_name must be an integer >= 1" >&2
    exit 2
  fi
done
if [[ ! "$cpu" =~ ^-?[0-9]+$ ]]; then
  echo "--cpu must be an integer" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"
if [[ "$skip_build" == 0 ]]; then
  if [[ "$(uname -s)/$(uname -m)" != "Darwin/arm64" ]]; then
    echo "standalone benchmark build currently requires arm64 macOS Apple container" >&2
    exit 2
  fi
  echo "==> Build standalone Cortex-A9 benchmark"
  container run --arch arm64 --rm --cpus 4 --memory 4g \
    --volume "$ROOT:/project" \
    --workdir /project \
    "$IMAGE" \
    arm-linux-gnueabihf-gcc \
      -std=c11 -O3 -Wall -Wextra -Werror \
      -march=armv7-a -mtune=cortex-a9 -mfpu=vfpv3-d16 -mfloat-abi=hard \
      -fno-tree-vectorize -ffunction-sections -fdata-sections \
      tools/rgb565-decimator-bench/main.c \
      magik-gui/src/framebuffer/downsample_scalar.c \
      -Wl,--gc-sections \
      -o /project/build/rgb565-decimator-bench/rgb565-decimator-bench
fi
if [[ ! -x "$BIN" ]]; then
  echo "missing benchmark binary: $BIN" >&2
  exit 2
fi

echo "==> Deploy standalone benchmark to $REMOTE_BIN"
"$ROOT/scripts/mister" put "$BIN" "$REMOTE_BIN" >/dev/null
"$ROOT/scripts/mister" run "chmod +x '$REMOTE_BIN'" >/dev/null

out="$OUT_DIR/${label}.tsv"
: >"$out"
for ((run = 1; run <= runs; run++)); do
  echo "==> Run $run/$runs samples=$samples cpu=$cpu"
  "$ROOT/scripts/mister" run \
    "'$REMOTE_BIN' --samples '$samples' --repeat '$run' --cpu '$cpu'" | tee -a "$out"
done

awk -F '\t' '
  $1 == "rgb565_decimator_bench_tsv" {
    delete value
    for (field = 2; field <= NF; field++) {
      split($field, pair, "=")
      value[pair[1]] = pair[2]
    }
    key = value["case"] SUBSEP value["kernel"]
    count[key]++
    p50[key] += value["p50_ns"]
    p95[key] += value["p95_ns"]
    if (value["max_ns"] > maximum[key]) maximum[key] = value["max_ns"]
    cases[value["case"]] = 1
    kernels[value["kernel"]] = 1
  }
  END {
    for (case_name in cases) {
      for (kernel_name in kernels) {
        key = case_name SUBSEP kernel_name
        if (count[key] > 0) {
          printf "rgb565_decimator_summary_tsv\tcase=%s\tkernel=%s\truns=%d\tmean_p50_ns=%.0f\tmean_p95_ns=%.0f\tmax_ns=%.0f\n", case_name, kernel_name, count[key], p50[key] / count[key], p95[key] / count[key], maximum[key]
        }
      }
    }
  }
' "$out" | sort | tee "$OUT_DIR/${label}-summary.tsv"

echo "wrote $out"
echo "wrote $OUT_DIR/${label}-summary.tsv"
