#!/usr/bin/env bash
# Benchmark debug-time Rust/Slint feedback loops without doing a full cargo clean.
#
# Examples:
#   scripts/bench-debug-build.sh --scenario arm-check-launcher --samples 3
#   scripts/bench-debug-build.sh --scenario all --samples 3 --state package-dirty
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT/magik-gui"
OUT="$ROOT/build/debug-build-bench.tsv"

SAMPLES=3
STATE=package-dirty
SCENARIO=all

usage() {
  cat <<'EOF'
Usage: scripts/bench-debug-build.sh [options]

Options:
  --scenario NAME   all | host-check | arm-check-lib | arm-check-launcher | arm-check-full |
                    arm-build-launcher | arm-build-full
  --samples N       samples per scenario (default: 3)
  --state STATE     warm | package-dirty (default: package-dirty)
  -h, --help        show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scenario) SCENARIO="${2:?}"; shift 2 ;;
    --samples) SAMPLES="${2:?}"; shift 2 ;;
    --state) STATE="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$STATE" in
  warm|package-dirty) ;;
  *) echo "unknown --state: $STATE" >&2; exit 2 ;;
esac

mkdir -p "$(dirname "$OUT")"
if [[ ! -f "$OUT" ]]; then
  printf 'date\tscenario\tsample\tstate\twall_sec\tcargo_sec\tbuild_script_run_sec\tlib_sec\tbin_sec\ttiming_report\tcommand\n' >"$OUT"
fi

latest_timing() {
  find "$RUST_DIR/target/cargo-timings" -maxdepth 1 -type f -name 'cargo-timing-*.html' -print 2>/dev/null \
    | sort \
    | tail -n 1
}

prepare_state() {
  if [[ "$STATE" == package-dirty ]]; then
    (cd "$RUST_DIR" && cargo clean -p mister-magic-fb --target armv7-unknown-linux-gnueabihf >/dev/null)
    (cd "$RUST_DIR" && cargo clean -p mister-magic-fb >/dev/null)
  fi
}

run_scenario() {
  local scenario="$1"
  case "$scenario" in
    host-check)
      (cd "$RUST_DIR" && cargo check --lib --no-default-features --timings --locked)
      ;;
    arm-check-launcher)
      "$ROOT/scripts/dev-rust" check-arm-ui
      ;;
    arm-check-lib)
      "$ROOT/scripts/dev-rust" check-arm-lib
      ;;
    arm-check-full)
      "$ROOT/scripts/dev-rust" check-arm-ui-full
      ;;
    arm-build-launcher)
      "$ROOT/scripts/dev-rust" build-arm-debug
      ;;
    arm-build-full)
      "$ROOT/scripts/dev-rust" build-arm-debug-full
      ;;
    *)
      echo "unknown scenario: $scenario" >&2
      exit 2
      ;;
  esac
}

summarize_timing() {
  local report="$1"
  python3 - "$report" <<'PY'
import json
import re
import sys

path = sys.argv[1]
if not path:
    print("\t\t")
    raise SystemExit

html = open(path, encoding="utf-8", errors="ignore").read()
m = re.search(r"<td>Total time:</td><td>([^<]+)</td>", html)
cargo_sec = ""
if m:
    text = m.group(1).split("(", 1)[0].strip()
    mm = re.fullmatch(r"(?:(\d+)m )?([\d.]+)s", text)
    if mm:
        cargo_sec = str(round((int(mm.group(1) or 0) * 60) + float(mm.group(2)), 2))

m = re.search(r"const UNIT_DATA = (\[.*?\]);", html, re.S)
build_script = lib = bin_ = ""
if m:
    units = json.loads(m.group(1))
    for unit in units:
        if unit.get("name") != "mister-magic-fb" or unit.get("duration", 0) <= 0:
            continue
        target = unit.get("target", "")
        duration = str(round(unit["duration"], 2))
        if "build-script (run)" in target:
            build_script = duration
        elif '"bin"' in target:
            bin_ = duration
        elif target in ("", " (check)"):
            lib = duration
print("\t".join([cargo_sec, build_script, lib, bin_]))
PY
}

scenarios=()
case "$SCENARIO" in
  all) scenarios=(host-check arm-check-lib arm-check-launcher arm-check-full arm-build-launcher arm-build-full) ;;
  host-check|arm-check-lib|arm-check-launcher|arm-check-full|arm-build-launcher|arm-build-full) scenarios=("$SCENARIO") ;;
  *) echo "unknown --scenario: $SCENARIO" >&2; exit 2 ;;
esac

for scenario in "${scenarios[@]}"; do
  for sample in $(seq 1 "$SAMPLES"); do
    echo "==> $scenario sample=$sample state=$STATE"
    prepare_state
    before="$(latest_timing)"
    start="$(date +%s)"
    run_scenario "$scenario"
    rc=$?
    end="$(date +%s)"
    [[ "$rc" -eq 0 ]] || exit "$rc"
    report="$(latest_timing)"
    if [[ "$report" == "$before" ]]; then
      report=""
    fi
    IFS=$'\t' read -r cargo_sec build_script_sec lib_sec bin_sec < <(summarize_timing "$report")
    command="$scenario"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      "$scenario" \
      "$sample" \
      "$STATE" \
      "$((end - start))" \
      "$cargo_sec" \
      "$build_script_sec" \
      "$lib_sec" \
      "$bin_sec" \
      "$report" \
      "$command" | tee -a "$OUT"
  done
done

echo "==> Results: $OUT"
