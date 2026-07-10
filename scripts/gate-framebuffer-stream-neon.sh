#!/usr/bin/env bash
# Prove RGB565 2x decimation dispatches through NEON on the MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/framebuffer-stream"

usage() {
  cat <<'USAGE'
Usage:
  scripts/gate-framebuffer-stream-neon.sh LABEL [--deploy-device]
  scripts/gate-framebuffer-stream-neon.sh --self-test

Runs the bench-tools-only framebuffer-stream-simd-bench command on the MiSTer.
The gate requires compiled and automatic NEON dispatch, identical scalar/NEON
output for contiguous, padded, and odd inputs, 960x540 half-snapshot NEON p95
at most 4ms/max at most 6ms, and at least 1.5x scalar speedup.
USAGE
}

check_gate_file() {
  local input="$1"
  awk -F '\t' '
    function fields(values,    field_index, pair) {
      delete values
      for (field_index = 2; field_index <= NF; field_index++) {
        split($field_index, pair, "=")
        values[pair[1]] = pair[2]
      }
    }

    $1 == "framebuffer_stream_simd_bench_tsv" {
      fields(value)
      key = value["case"] SUBSEP value["implementation"]
      rows[key] += 1
      checksum[key] = value["checksum"]
    }

    $1 == "framebuffer_stream_simd_gate_tsv" {
      fields(gate)
      gate_rows += 1
    }

    END {
      expected[1] = "full_960x540"
      expected[2] = "padded_960x540"
      expected[3] = "odd_959x539"
      failed = 0

      if (gate_rows != 1) {
        print "ERROR: expected exactly one framebuffer_stream_simd_gate_tsv row" > "/dev/stderr"
        failed = 1
      }
      for (case_index = 1; case_index <= 3; case_index++) {
        name = expected[case_index]
        scalar_key = name SUBSEP "scalar"
        neon_key = name SUBSEP "neon"
        if (rows[scalar_key] != 1 || rows[neon_key] != 1) {
          print "ERROR: missing scalar/NEON rows for " name > "/dev/stderr"
          failed = 1
        } else if (checksum[scalar_key] != checksum[neon_key]) {
          print "ERROR: scalar/NEON checksum mismatch for " name > "/dev/stderr"
          failed = 1
        }
      }
      if (gate["compiled_implementation"] != "neon") {
        print "ERROR: ARM binary did not compile the NEON implementation" > "/dev/stderr"
        failed = 1
      }
      if (gate["auto_implementation"] != "neon") {
        print "ERROR: auto dispatch did not select NEON" > "/dev/stderr"
        failed = 1
      }
      if (gate["checksums_identical"] != 1) {
        print "ERROR: benchmark summary reports mismatched output" > "/dev/stderr"
        failed = 1
      }
      if ((gate["half_snapshot_neon_p95_us"] + 0) > 4000) {
        print "ERROR: NEON p95 exceeds 4ms" > "/dev/stderr"
        failed = 1
      }
      if ((gate["half_snapshot_neon_max_us"] + 0) > 6000) {
        print "ERROR: NEON max exceeds 6ms" > "/dev/stderr"
        failed = 1
      }
      if ((gate["speedup"] + 0) < 1.5) {
        print "ERROR: NEON speedup is below 1.5x" > "/dev/stderr"
        failed = 1
      }
      if (gate["passed"] != 1) {
        print "ERROR: device benchmark did not pass its internal gate" > "/dev/stderr"
        failed = 1
      }

      if (!failed) {
        printf "framebuffer_stream_neon_gate_tsv\timplementation=neon\tp95_us=%s\tmax_us=%s\tspeedup=%s\tpassed=1\n", gate["half_snapshot_neon_p95_us"], gate["half_snapshot_neon_max_us"], gate["speedup"]
      }
      exit failed
    }
  ' "$input"
}

run_self_test() {
  local valid invalid
  valid="$(mktemp)"
  invalid="$(mktemp)"
  trap 'rm -f "$valid" "$invalid"' RETURN

  printf '%s\n' \
    $'framebuffer_stream_simd_bench_tsv\tcase=full_960x540\timplementation=scalar\tchecksum=aa' \
    $'framebuffer_stream_simd_bench_tsv\tcase=full_960x540\timplementation=neon\tchecksum=aa' \
    $'framebuffer_stream_simd_bench_tsv\tcase=padded_960x540\timplementation=scalar\tchecksum=bb' \
    $'framebuffer_stream_simd_bench_tsv\tcase=padded_960x540\timplementation=neon\tchecksum=bb' \
    $'framebuffer_stream_simd_bench_tsv\tcase=odd_959x539\timplementation=scalar\tchecksum=cc' \
    $'framebuffer_stream_simd_bench_tsv\tcase=odd_959x539\timplementation=neon\tchecksum=cc' \
    $'framebuffer_stream_simd_gate_tsv\tcompiled_implementation=neon\tauto_implementation=neon\tchecksums_identical=1\thalf_snapshot_neon_p95_us=3900\thalf_snapshot_neon_max_us=5900\thalf_snapshot_scalar_p95_us=6000\tspeedup=1.538\tpassed=1' \
    >"$valid"
  check_gate_file "$valid" >/dev/null

  sed 's/speedup=1.538/speedup=1.499/; s/passed=1/passed=0/' "$valid" >"$invalid"
  if check_gate_file "$invalid" >/dev/null 2>&1; then
    echo "ERROR: sub-threshold speedup fixture unexpectedly passed" >&2
    return 1
  fi
  echo "framebuffer stream NEON gate self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
  run_self_test
  exit 0
fi

label="${1:-}"
if [[ -z "$label" || "$label" == "-h" || "$label" == "--help" ]]; then
  usage
  [[ -z "$label" ]] && exit 2 || exit 0
fi
shift

deploy_device=0
while (($#)); do
  case "$1" in
    --deploy-device) deploy_device=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unexpected argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi

if [[ "$deploy_device" == 1 ]]; then
  "$HERE/scripts/deploy-rust.sh" --device --bench-tools --ui-scope launcher
fi

mkdir -p "$OUT_DIR"
local_log="$OUT_DIR/${label}-neon.tsv"
remote_log="/tmp/${label}-framebuffer-stream-neon.tsv"

set +e
"$MISTER" run "rm -f '$remote_log'; /media/fat/mister-magik/mister-magik-fb framebuffer-stream-simd-bench >'$remote_log' 2>&1" >/dev/null
command_status=$?
set -e
"$MISTER" get "$remote_log" "$local_log" >/dev/null

if ! check_gate_file "$local_log"; then
  echo "NEON gate failed; evidence: $local_log" >&2
  exit 1
fi
if [[ "$command_status" != 0 ]]; then
  echo "NEON benchmark returned status $command_status; evidence: $local_log" >&2
  exit "$command_status"
fi
echo "wrote $local_log"
