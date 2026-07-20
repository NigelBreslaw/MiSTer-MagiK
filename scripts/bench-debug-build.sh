#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Measure local Rust/ARM build iteration time for the MiSTer Slint frontend.
#
# Raw run logs and TSV rows are written under build/ (gitignored). Curated
# conclusions belong in history/toolchain-bench/compile-time-experiments-*.md.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$ROOT/apps/mister"
OUT_DIR="$ROOT/build"
LOG_DIR="$OUT_DIR/debug-build-logs"
TSV="$OUT_DIR/debug-build-bench.tsv"

scenario="arm-check-launcher"
state="noop-warm"
samples=3
warmups=1
label=""

usage() {
  cat <<'EOF'
Usage: scripts/bench-debug-build.sh [options]

Options:
  --scenario NAME   all | arm-check-lib | arm-check-launcher | arm-check-arcade |
                    arm-check-full | arm-build-launcher | arm-build-arcade |
                    arm-build-full |
                    build-ui-fast | build-ui-device
  --state NAME      noop-warm | touch-rust-bin | touch-rust-lib |
                    touch-rust-catalog | touch-rust-core |
                    touch-rust-platform | touch-slint-launcher |
                    touch-slint-shared | touch-build-rs | clean
  --samples N       measured samples per command (default: 3)
  --warmups N       warm-up runs before measured samples (default: 1)
  --label LABEL     label written to the TSV (default: timestamp + scenario/state)
  -h, --help        show this help

Results:
  build/debug-build-bench.tsv
  build/debug-build-logs/*.log

Notes:
  The state mutators only touch mtimes or ignored build output. They do not edit
  tracked file contents. The clean state removes target output for the measured
  ARM target and should be used deliberately.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --scenario) scenario="${2:?missing scenario}"; shift 2 ;;
    --state) state="${2:?missing state}"; shift 2 ;;
    --samples) samples="${2:?missing samples}"; shift 2 ;;
    --warmups) warmups="${2:?missing warmups}"; shift 2 ;;
    --label) label="${2:?missing label}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "ERROR: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$samples" in ''|*[!0-9]*) echo "ERROR: --samples must be an integer" >&2; exit 2 ;; esac
case "$warmups" in ''|*[!0-9]*) echo "ERROR: --warmups must be an integer" >&2; exit 2 ;; esac

mkdir -p "$OUT_DIR" "$LOG_DIR"

state_path() {
  case "$state" in
    touch-rust-bin) echo "$RUST_DIR/src/ui_runner.rs" ;;
    touch-rust-lib) echo "$RUST_DIR/src/launcher.rs" ;;
    touch-rust-catalog) echo "$ROOT/crates/catalog/src/arcade_catalog.rs" ;;
    touch-rust-core) echo "$ROOT/crates/magik-core/src/input_state.rs" ;;
    touch-rust-platform) echo "$ROOT/mister/platform/runtime/src/framebuffer/mod.rs" ;;
    touch-slint-launcher) echo "$RUST_DIR/ui/launcher.slint" ;;
    touch-slint-shared) echo "$RUST_DIR/ui/mister_window.slint" ;;
    touch-build-rs) echo "$RUST_DIR/build.rs" ;;
  esac
}

restore_path="$(state_path)"
restore_stamp=""
if [ -n "$restore_path" ]; then
  restore_stamp="$(mktemp "${TMPDIR:-/tmp}/mister-magik-build-bench-stamp.XXXXXX")"
  touch -r "$restore_path" "$restore_stamp"
fi
cleanup() {
  if [ -n "$restore_stamp" ] && [ -e "$restore_stamp" ]; then
    touch -r "$restore_stamp" "$restore_path"
    rm -f "$restore_stamp"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

ensure_tsv_header() {
  local expected
  expected="$(tsv_header)"
  if [ -f "$TSV" ] && [ "$(head -n 1 "$TSV")" != "$expected" ]; then
    local legacy="$OUT_DIR/debug-build-bench.legacy.$(date -u +%Y%m%dT%H%M%SZ).tsv"
    mv "$TSV" "$legacy"
    echo "==> moved legacy benchmark TSV to $legacy"
  fi
}

tsv_header() {
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s' \
    label timestamp git_rev dirty scenario state command sample warmup exit_status \
    wall_sec cargo_total_sec app_unit_sec top_units timing_html binary_bytes log_path
}

ensure_tsv_header
if [ ! -f "$TSV" ]; then
  tsv_header >"$TSV"
  printf '\n' >>"$TSV"
fi

now_sec() {
  perl -MTime::HiRes=time -e 'printf "%.3f", time'
}

timestamp() {
  date -u +"%Y-%m-%dT%H:%M:%SZ"
}

git_rev() {
  git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown
}

dirty_marker() {
  if git -C "$ROOT" diff --quiet --ignore-submodules -- && \
     git -C "$ROOT" diff --cached --quiet --ignore-submodules --; then
    echo clean
  else
    echo dirty
  fi
}

safe_label_part() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9_.-' '_'
}

command_spec() {
  case "$1" in
    arm-check-lib) echo "scripts/agent arm check-lib" ;;
    arm-check-launcher) echo "scripts/agent arm check-launcher" ;;
    arm-check-arcade) echo "scripts/agent arm check-arcade" ;;
    arm-check-full) echo "scripts/agent arm check-all" ;;
    arm-build-launcher|arm-build-arcade|arm-build-full) echo "scripts/agent arm build-device" ;;
    build-ui-device) echo "apps/mister/build-arm.sh --device" ;;
    build-ui-fast) echo "apps/mister/build-arm.sh --fast" ;;
    *) return 1 ;;
  esac
}

scenario_commands() {
  case "$scenario" in
    all)
      printf '%s\n' \
        arm-check-lib \
        arm-check-launcher \
        arm-check-arcade \
        arm-check-full \
        arm-build-launcher \
        build-ui-fast \
        build-ui-device
      ;;
    arm-check-lib|arm-check-launcher|arm-check-arcade|arm-check-full|\
    arm-build-launcher|arm-build-arcade|arm-build-full|\
    build-ui-fast|build-ui-device)
      printf '%s\n' "$scenario"
      ;;
    *)
      echo "ERROR: unknown scenario: $scenario" >&2
      usage >&2
      exit 2
      ;;
  esac
}

apply_state() {
  case "$state" in
    noop-warm)
      ;;
    touch-rust-bin)
      touch "$RUST_DIR/src/ui_runner.rs"
      ;;
    touch-rust-lib)
      touch "$RUST_DIR/src/launcher.rs"
      ;;
    touch-rust-catalog)
      touch "$ROOT/crates/catalog/src/arcade_catalog.rs"
      ;;
    touch-rust-core)
      touch "$ROOT/crates/magik-core/src/input_state.rs"
      ;;
    touch-rust-platform)
      touch "$ROOT/mister/platform/runtime/src/framebuffer/mod.rs"
      ;;
    touch-slint-launcher)
      touch "$RUST_DIR/ui/launcher.slint"
      ;;
    touch-slint-shared)
      touch "$RUST_DIR/ui/mister_window.slint"
      ;;
    touch-build-rs)
      touch "$RUST_DIR/build.rs"
      ;;
    clean)
      rm -rf "$RUST_DIR/target/armv7-unknown-linux-gnueabihf"
      rm -rf "$RUST_DIR/target/cargo-timings"
      ;;
    *)
      echo "ERROR: unknown state: $state" >&2
      usage >&2
      exit 2
      ;;
  esac
}

latest_timing_html() {
  find "$RUST_DIR/target/cargo-timings" -type f -name 'cargo-timing*.html' \
    -print 2>/dev/null | xargs ls -t 2>/dev/null | head -n 1 || true
}

extract_html_cell_after() {
  local pattern="$1"
  local file="$2"
  awk -v pat="$pattern" '
    index($0, pat) {
      if (match($0, /<td>[^<]*<\/td><td>[^<]*/)) {
        s = substr($0, RSTART, RLENGTH)
        sub(/^<td>[^<]*<\/td><td>/, "", s)
        print s
        exit
      }
    }
  ' "$file"
}

extract_total_sec() {
  local file="$1"
  local value
  value="$(extract_html_cell_after "Total time:" "$file" | sed 's/s$//')"
  printf '%s' "$value"
}

extract_app_unit_sec() {
  local file="$1"
  awk '
    /mister-magik-fb v0\.1\.0/ && /mister-magik-fb "&quot;bin&quot;"|mister-magik-fb "bin"/ {
      for (i = 0; i < 8; i++) {
        if (getline <= 0) exit
        if ($0 ~ /<td>[0-9.]+s<\/td>/) {
          gsub(/.*<td>/, "", $0)
          gsub(/s<\/td>.*/, "", $0)
          print
          exit
        }
      }
    }
  ' "$file"
}

extract_top_units() {
  local file="$1"
  awk '
    BEGIN { rows = 0; capture = 0; name = ""; dur = "" }
    /<tbody>/ { capture = 1 }
    capture && /<td>[0-9]+\.<\/td>/ { name = ""; dur = ""; next }
    capture && name == "" && /<td>.*<\/td>/ {
      line = $0
      gsub(/^[[:space:]]*<td>/, "", line)
      gsub(/<\/td>.*/, "", line)
      if (line !~ /^[0-9.]+s$/ && line !~ /^[0-9]+.$/) {
        name = line
        next
      }
    }
    capture && name != "" && dur == "" && /<td>[0-9.]+s<\/td>/ {
      line = $0
      gsub(/^[[:space:]]*<td>/, "", line)
      gsub(/s<\/td>.*/, "", line)
      dur = line
      if (dur + 0 > 0) {
        gsub(/\t/, " ", name)
        printf "%s=%ss;", name, dur
        rows++
      }
      name = ""; dur = ""
      if (rows >= 8) exit
    }
  ' "$file"
}

extract_finished_sec_from_log() {
  local file="$1"
  awk '
    /Finished .* target\(s\) in / {
      line = $0
      sub(/^.* target\(s\) in /, "", line)
      sub(/[[:space:]]*$/, "", line)
      if (line ~ /^[0-9.]+s$/) {
        sub(/s$/, "", line)
        value = line
      } else if (line ~ /^[0-9]+m [0-9.]+s$/) {
        split(line, parts, "m ")
        minutes = parts[1] + 0
        seconds = parts[2]
        sub(/s$/, "", seconds)
        value = (minutes * 60) + seconds
      }
    }
    END { if (value != "") print value }
  ' "$file"
}

binary_size_for_command() {
  local cmd="$1"
  local profile=""
  case "$cmd" in
    build-ui-fast) profile="release" ;;
    build-ui-device) profile="release-device" ;;
    *) profile="debug" ;;
  esac

  local bin="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/$profile/mister-magik-fb"
  if [ -f "$bin" ]; then
    wc -c <"$bin" | tr -d ' '
  fi
}

run_one() {
  local cmd_name="$1"
  local sample="$2"
  local is_warmup="$3"
  local cmd
  cmd="$(command_spec "$cmd_name")" || {
    echo "ERROR: unknown command scenario: $cmd_name" >&2
    exit 2
  }

  apply_state

  local run_label="$label"
  if [ -z "$run_label" ]; then
    run_label="$(date -u +%Y%m%dT%H%M%SZ)-$scenario-$state"
  fi

  local safe_label safe_cmd log start end wall status prev_timing timing total app_unit top_units bytes
  safe_label="$(safe_label_part "$run_label")"
  safe_cmd="$(safe_label_part "$cmd_name")"
  log="$LOG_DIR/${safe_label}-${safe_cmd}-sample${sample}-warmup${is_warmup}.log"

  echo "==> [$cmd_name] sample=$sample warmup=$is_warmup state=$state"
  prev_timing="$(latest_timing_html)"
  start="$(now_sec)"
  set +e
  (
    cd "$ROOT"
    eval "$cmd"
  ) >"$log" 2>&1
  status=$?
  set -e
  end="$(now_sec)"
  wall="$(perl -e 'printf "%.3f", $ARGV[1] - $ARGV[0]' "$start" "$end")"

  timing="$(latest_timing_html)"
  if [ "$timing" = "$prev_timing" ]; then
    timing=""
  fi
  total=""
  app_unit=""
  top_units=""
  if [ -n "$timing" ] && [ -f "$timing" ]; then
    total="$(extract_total_sec "$timing")"
    app_unit="$(extract_app_unit_sec "$timing")"
    top_units="$(extract_top_units "$timing")"
  fi
  if [ -z "$total" ] || [ "$total" = "0.0" ] || [ "$total" = "0" ]; then
    total="$(extract_finished_sec_from_log "$log")"
  fi
  bytes="$(binary_size_for_command "$cmd_name")"

  if [ "$is_warmup" = 0 ]; then
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$run_label" "$(timestamp)" "$(git_rev)" "$(dirty_marker)" "$scenario" "$state" \
      "$cmd_name" "$sample" "$is_warmup" "$status" "$wall" "$total" "$app_unit" \
      "$top_units" "$timing" "$bytes" "$log" >>"$TSV"
  fi

  awk '
    /(^|[[:space:]])Compiling / { compiling++ }
    /(^|[[:space:]])Checking / { checking++ }
    /(^|[ \/])cargo (build|check|test|clippy|fmt)|container build profile=/ { cargo_runs++ }
    /^WORKFLOW_COUNT kind=container / { container_runs++ }
    /^WORKFLOW_COUNT kind=ffmpeg-cache-check / { ffmpeg_runs++ }
    END {
      printf "BUILD_BENCH_COUNTS compiled=%d checked=%d cargo_runs=%d container_runs=%d ffmpeg_runs=%d\n",
        compiling + 0, checking + 0, cargo_runs + 0, container_runs + 0, ffmpeg_runs + 0
    }
  ' "$log"
  grep '^WORKFLOW_PHASE ' "$log" || true

  if [ "$status" -ne 0 ]; then
    echo "ERROR: command failed: $cmd" >&2
    echo "       log: $log" >&2
    exit "$status"
  fi
}

mapfile -t commands < <(scenario_commands)

for cmd_name in "${commands[@]}"; do
  for ((i = 1; i <= warmups; i++)); do
    run_one "$cmd_name" "$i" 1
  done
  for ((i = 1; i <= samples; i++)); do
    run_one "$cmd_name" "$i" 0
  done
done

echo "==> wrote $TSV"
