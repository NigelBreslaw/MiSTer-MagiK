#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Measure the local Git, validation, ARM build, and deploy iteration workflow.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

tier=quick
group=all
cold=0
device=0
label=""
self_test=0
samples_override=""
warmups_override=""
scenario_filter=""

usage() {
  cat <<'EOF'
Usage: scripts/bench-local-workflow.sh [options]

Options:
  --tier quick|full                   adaptive quick or repeat-sampled run
  --group git|precommit|validate|build|deploy|all
  --cold                              use a benchmark-owned empty ARM target
  --device                            include bounded real MiSTer deployment
  --label LABEL                       human-readable run label
  --samples N                         override measured sample count
  --warmups N                         override warm-up count
  --scenario NAME                     run one validation/hook scenario
  --self-test                         run host-local parser/safety tests

Output is written below build/local-workflow-bench/<run-id>/.
The default run never contacts the MiSTer or changes tracked contents/index.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --tier) tier="${2:?--tier requires quick or full}"; shift 2 ;;
    --group) group="${2:?--group requires a name}"; shift 2 ;;
    --cold) cold=1; shift ;;
    --device) device=1; shift ;;
    --label) label="${2:?--label requires text}"; shift 2 ;;
    --samples) samples_override="${2:?--samples requires an integer}"; shift 2 ;;
    --warmups) warmups_override="${2:?--warmups requires an integer}"; shift 2 ;;
    --scenario) scenario_filter="${2:?--scenario requires a name}"; shift 2 ;;
    --self-test) self_test=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$tier" in quick|full) ;; *) echo "ERROR: invalid tier: $tier" >&2; exit 2 ;; esac
case "$group" in git|precommit|validate|build|deploy|all) ;; *) echo "ERROR: invalid group: $group" >&2; exit 2 ;; esac
case "$samples_override" in "") ;; *[!0-9]*) echo "ERROR: --samples must be an integer" >&2; exit 2 ;; esac
case "$warmups_override" in "") ;; *[!0-9]*) echo "ERROR: --warmups must be an integer" >&2; exit 2 ;; esac

safe_name() { printf '%s' "$1" | tr -c 'A-Za-z0-9_.-' '_'; }
if [ "$self_test" -eq 1 ]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
cat >"$tmp/results.tsv" <<'EOF'
group	scenario	command	sample	warmup	status	expected_status	status_ok	wall_ms	git_trace_ms	harness_overhead_ms	user_ms	system_ms	max_rss_kb	compiled	checked	cargo_runs	container_runs	ffmpeg_runs	validation_checks	trace_path	log_path	cache_state
git	status	git status	1	0	0	0	1	10	8	2	2	3	100	0	0	0	0	0	0		/a	warm
git	status	git status	2	0	0	0	1	30	9	21	4	5	120	0	0	0	0	0	0		/b	warm
build	noop	build	1	0	0	0	1	50	0	0	1	2	130	2	1	2	2	1	0		/c	warm
EOF
  python3 - "$tmp/results.tsv" <<'PY'
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
values = [int(r["wall_ms"]) for r in rows if r["scenario"] == "status"]
assert statistics.median(values) == 20
assert max(values) == 30 and min(values) == 10
build = next(r for r in rows if r["group"] == "build")
assert int(build["cargo_runs"]) == 2 and int(build["container_runs"]) == 2
assert int(build["compiled"]) + int(build["checked"]) == 3
assert all(r["status_ok"] == "1" for r in rows)
assert [int(r["harness_overhead_ms"]) for r in rows if r["group"] == "git"] == [2, 21]
PY
  idx="$tmp/index"
  GIT_INDEX_FILE="$idx" git read-tree HEAD
  before="$(git status --porcelain=v1)"
  mode="$(git ls-tree HEAD -- scripts/README.md | awk '{print $1}')"
  blob="$(git ls-tree HEAD -- AGENTS.md | awk '{print $3}')"
  GIT_INDEX_FILE="$idx" git update-index --add --cacheinfo "$mode,$blob,scripts/README.md"
  GIT_INDEX_FILE="$idx" git diff --cached --quiet && { echo "self-test synthetic index did not differ" >&2; exit 1; }
  [ "$(git status --porcelain=v1)" = "$before" ] || { echo "self-test changed the real index/worktree" >&2; exit 1; }
  stamp="$tmp/stamp"
  touch -r scripts/README.md "$stamp"
  touch scripts/README.md
  touch -r "$stamp" scripts/README.md
  [ "$(stat -f %m scripts/README.md 2>/dev/null || stat -c %Y scripts/README.md)" = "$(stat -f %m "$stamp" 2>/dev/null || stat -c %Y "$stamp")" ] || exit 1
  cold_dir="$tmp/cold-target"
  mkdir -p "$cold_dir/sentinel"
  rm -rf "$cold_dir"
  [ ! -e "$cold_dir" ] && [ -e "$tmp/index" ] || exit 1
  set +e
  MISTER_LOCAL_WORKFLOW_INTERRUPT_SELF_TEST=1 "$0" --group git --samples 1 --warmups 0 --label interrupt-self-test >"$tmp/interrupt.log" 2>&1 &
  interrupt_pid=$!
  sleep 0.5
  kill -TERM "$interrupt_pid"
  wait "$interrupt_pid"
  interrupt_status=$?
  set -e
  [ "$interrupt_status" -eq 143 ] || {
    echo "interrupt self-test returned $interrupt_status, expected 143" >&2
    cat "$tmp/interrupt.log" >&2
    exit 1
  }
  if pgrep -f 'mister-local-workflow-interrupt-child' >/dev/null 2>&1; then
    echo "interrupt self-test left a child process running" >&2
    exit 1
  fi
  MISTER_LOCAL_WORKFLOW_EXPECTED_SELF_TEST=1 "$0" --group git --samples 1 --warmups 0 --label expected-status-self-test >"$tmp/expected.log" 2>&1 || {
    cat "$tmp/expected.log" >&2
    exit 1
  }
  printf 'local workflow benchmark self-test: pass\n'
  exit 0
fi

run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
[ -z "$label" ] && label="$run_id"
out="$ROOT/build/local-workflow-bench/$run_id"
logs="$out/logs"
traces="$out/traces"
mkdir -p "$logs" "$traces"
results="$out/results.tsv"
printf 'group\tscenario\tcommand\tsample\twarmup\tstatus\texpected_status\tstatus_ok\twall_ms\tgit_trace_ms\tharness_overhead_ms\tuser_ms\tsystem_ms\tmax_rss_kb\tcompiled\tchecked\tcargo_runs\tcontainer_runs\tffmpeg_runs\tvalidation_checks\ttrace_path\tlog_path\tcache_state\n' >"$results"

real_index_fingerprint="$(git rev-parse --git-path index | xargs shasum -a 256 2>/dev/null | awk '{print $1}' || true)"
restore_files=()
restore_stamps=()
active_pid=""
guard_dir="$out/tracked-state-before"
mkdir -p "$guard_dir"
guard_files=(crates/catalog/Cargo.lock mister/tools/host/Cargo.lock)
for guard_file in "${guard_files[@]}"; do
  mkdir -p "$guard_dir/$(dirname "$guard_file")"
  cp -p "$guard_file" "$guard_dir/$guard_file"
done
cleanup() {
  local i current guard_file child
  if [ -n "$active_pid" ]; then
    mapfile -t active_descendants < <(descendants_of "$active_pid")
    for child in "${active_descendants[@]}"; do kill -TERM "$child" 2>/dev/null || true; done
    kill -TERM "$active_pid" 2>/dev/null || true
    sleep 0.2
    for child in "${active_descendants[@]}"; do kill -KILL "$child" 2>/dev/null || true; done
    kill -KILL "$active_pid" 2>/dev/null || true
    active_pid=""
  fi
  for ((i=0; i<${#restore_files[@]}; i++)); do
    [ -e "${restore_stamps[$i]}" ] && touch -r "${restore_stamps[$i]}" "${restore_files[$i]}"
  done
  : >"$out/tracked-state-contamination.tsv"
  printf 'path\trestored\n' >"$out/tracked-state-contamination.tsv"
  for guard_file in "${guard_files[@]}"; do
    if ! cmp -s "$guard_file" "$guard_dir/$guard_file"; then
      cp -p "$guard_dir/$guard_file" "$guard_file"
      printf '%s\t1\n' "$guard_file" >>"$out/tracked-state-contamination.tsv"
    fi
  done
  current="$(git rev-parse --git-path index | xargs shasum -a 256 2>/dev/null | awk '{print $1}' || true)"
  if [ -n "$real_index_fingerprint" ] && [ "$current" != "$real_index_fingerprint" ]; then
    echo "WARNING: real Git index fingerprint changed during benchmark" >&2
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

descendants_of() {
  local parent="$1" child
  while IFS= read -r child; do
    [ -n "$child" ] || continue
    descendants_of "$child"
    echo "$child"
  done < <(pgrep -P "$parent" 2>/dev/null || true)
}

warmups="${warmups_override:-0}"
if [ -z "$warmups_override" ] && [ "$tier" = full ]; then warmups=1; fi

samples_for_group() {
  local measured_group="$1"
  if [ -n "$samples_override" ]; then
    echo "$samples_override"
  elif [ "$tier" = quick ]; then
    echo 1
  else
    case "$measured_group" in build|deploy) echo 2 ;; *) echo 5 ;; esac
  fi
}

cat >"$out/environment.tsv" <<EOF
key\tvalue
label\t$label
tier\t$tier
group\t$group
cold\t$cold
device\t$device
timestamp_utc\t$(date -u +%Y-%m-%dT%H:%M:%SZ)
git_revision\t$(git rev-parse HEAD)
dirty\t$([ -n "$(git status --porcelain=v1)" ] && echo 1 || echo 0)
host\t$(hostname)
os\t$(uname -srv)
arch\t$(uname -m)
cpu_count\t$(sysctl -n hw.ncpu 2>/dev/null || getconf _NPROCESSORS_ONLN)
memory_bytes\t$(sysctl -n hw.memsize 2>/dev/null || echo unknown)
filesystem\t$(df -T . 2>/dev/null | tail -1 || df . | tail -1)
git_version\t$(git --version)
cargo_version\t$(cargo --version 2>/dev/null || echo unavailable)
container_version\t$(container --version 2>/dev/null | head -1 || echo unavailable)
container_builder\t$(container builder status 2>/dev/null | tr '\n' ' ' || echo unavailable)
EOF

parse_time_file() {
  local file="$1" field="$2"
  case "$field" in
    real) awk '/^real /{print int($2 * 1000 + 0.5)}' "$file" ;;
    user) awk '/^user /{print int($2 * 1000 + 0.5)}' "$file" ;;
    system) awk '/^sys /{print int($2 * 1000 + 0.5)}' "$file" ;;
    rss) echo 0 ;;
  esac
}

git_trace_duration_ms() {
  local trace="$1"
  [ -n "$trace" ] && [ -f "$trace" ] || { echo 0; return; }
  python3 - "$trace" <<'PY'
import json, sys
duration = 0.0
for line in open(sys.argv[1], errors="replace"):
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        continue
    if event.get("event") == "exit":
        duration = max(duration, float(event.get("t_abs", 0.0)))
print(round(duration * 1000))
PY
}

record_command() {
  local grp="$1" scenario="$2" command_label="$3" sample="$4" warmup="$5" trace="$6" expected_status="$7"
  shift 7
  local stem log timing status expected_status status_ok wall git_ms overhead user system rss compiled checked cargo_runs container_runs ffmpeg_runs checks
  stem="$(safe_name "$grp-$scenario-$sample-w$warmup")"
  log="$logs/$stem.log"
  timing="$logs/$stem.time"
  set +e
  if [ -n "$trace" ]; then
    GIT_TRACE2_EVENT="$trace" /usr/bin/time -p "$@" >"$log" 2>"$timing" &
  else
    /usr/bin/time -p "$@" >"$log" 2>"$timing" &
  fi
  active_pid=$!
  wait "$active_pid"
  status=$?
  active_pid=""
  set -e
  wall="$(parse_time_file "$timing" real)"; wall="${wall:-0}"
  git_ms="$(git_trace_duration_ms "$trace")"
  overhead=0
  [ "$git_ms" -eq 0 ] || overhead=$((wall - git_ms))
  [ "$overhead" -ge 0 ] || overhead=0
  status_ok=0
  [ "$status" -eq "$expected_status" ] && status_ok=1
  user="$(parse_time_file "$timing" user)"; user="${user:-0}"
  system="$(parse_time_file "$timing" system)"; system="${system:-0}"
  rss="$(parse_time_file "$timing" rss)"; rss="${rss:-0}"
  compiled="$(awk '/(^|[[:space:]])Compiling / { n++ } END { print n + 0 }' "$log" "$timing")"
  checked="$(awk '/(^|[[:space:]])Checking / { n++ } END { print n + 0 }' "$log" "$timing")"
  cargo_runs="$(awk '/(^|[ \/])cargo (build|check|test|clippy|fmt)|container build profile=/ { n++ } END { print n + 0 }' "$log" "$timing")"
  container_runs="$(awk '/container run|image arch probe/ { n++ } END { print n + 0 }' "$log" "$timing")"
  ffmpeg_runs="$(awk '/minimal FFmpeg|build-minimal-ffmpeg/ { n++ } END { print n + 0 }' "$log" "$timing")"
  checks="$(awk '/VALIDATION start check=/ { n++ } END { print n + 0 }' "$log" "$timing")"
  if grep -q '^BUILD_BENCH_COUNTS ' "$log"; then
    compiled="$(awk -F'[ =]' '/^BUILD_BENCH_COUNTS / { for (i=1;i<=NF;i++) if ($i=="compiled") n+=$(i+1) } END { print n+0 }' "$log")"
    checked="$(awk -F'[ =]' '/^BUILD_BENCH_COUNTS / { for (i=1;i<=NF;i++) if ($i=="checked") n+=$(i+1) } END { print n+0 }' "$log")"
    cargo_runs="$(awk -F'[ =]' '/^BUILD_BENCH_COUNTS / { for (i=1;i<=NF;i++) if ($i=="cargo_runs") n+=$(i+1) } END { print n+0 }' "$log")"
    container_runs="$(awk -F'[ =]' '/^BUILD_BENCH_COUNTS / { for (i=1;i<=NF;i++) if ($i=="container_runs") n+=$(i+1) } END { print n+0 }' "$log")"
    ffmpeg_runs="$(awk -F'[ =]' '/^BUILD_BENCH_COUNTS / { for (i=1;i<=NF;i++) if ($i=="ffmpeg_runs") n+=$(i+1) } END { print n+0 }' "$log")"
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$grp" "$scenario" "$command_label" "$sample" "$warmup" "$status" "$expected_status" "$status_ok" "$wall" "$git_ms" "$overhead" "$user" "$system" "$rss" \
    "$compiled" "$checked" "$cargo_runs" "$container_runs" "$ffmpeg_runs" "$checks" "$trace" "$log" "$([ "$cold" -eq 1 ] && echo cold-owned || echo warm-shared)" >>"$results"
  RECORDED_STATUS="$status"
  return 0
}

repeat_command() {
  local grp="$1" scenario="$2" command_label="$3" trace_kind="$4"; shift 4
  local n trace command_samples
  command_samples="$(samples_for_group "$grp")"
  for ((n=1; n<=warmups; n++)); do
    trace=""; [ "$trace_kind" = git ] && trace="$traces/$(safe_name "$grp-$scenario-warmup$n").json"
    record_command "$grp" "$scenario" "$command_label" "$n" 1 "$trace" 0 "$@"
  done
  for ((n=1; n<=command_samples; n++)); do
    trace=""; [ "$trace_kind" = git ] && trace="$traces/$(safe_name "$grp-$scenario-sample$n").json"
    record_command "$grp" "$scenario" "$command_label" "$n" 0 "$trace" 0 "$@"
  done
}

device_command() {
  local scenario="$1" command_label="$2"; shift 2
  local n command_samples
  command_samples="$(samples_for_group deploy)"
  for ((n=1; n<=warmups; n++)); do
    record_command deploy "$scenario" "$command_label" "$n" 1 "" 0 "$@"
    [ "$RECORDED_STATUS" -eq 0 ] || return "$RECORDED_STATUS"
  done
  for ((n=1; n<=command_samples; n++)); do
    record_command deploy "$scenario" "$command_label" "$n" 0 "" 0 "$@"
    [ "$RECORDED_STATUS" -eq 0 ] || return "$RECORDED_STATUS"
  done
}

synthetic_index() {
  local target="$1" idx="$2" mode blob
  rm -f "$idx"
  GIT_INDEX_FILE="$idx" git read-tree HEAD
  mode="$(git ls-tree HEAD -- "$target" | awk '{print $1}')"
  [ -n "$mode" ] || mode=100644
  blob="$(git ls-tree HEAD -- AGENTS.md | awk '{print $3}')"
  [ -n "$blob" ] || { echo "ERROR: cannot locate alternate fixture blob" >&2; exit 1; }
  GIT_INDEX_FILE="$idx" git update-index --add --cacheinfo "$mode,$blob,$target"
}

run_git_group() {
  local idx="$out/git-index"
  synthetic_index scripts/README.md "$idx"
  repeat_command git status-current 'git status --short' git git status --short
  repeat_command git diff-working 'git diff' git git diff --no-ext-diff
  repeat_command git diff-cached-real 'git diff --cached' git git diff --cached --no-ext-diff
  repeat_command git status-synthetic 'GIT_INDEX_FILE git status --short' git env GIT_INDEX_FILE="$idx" git status --short
  repeat_command git diff-cached-synthetic 'GIT_INDEX_FILE git diff --cached' git env GIT_INDEX_FILE="$idx" git diff --cached --no-ext-diff
  repeat_command git discovery-staged 'validate staged-path discovery' git env GIT_INDEX_FILE="$idx" git diff --cached --name-status -z --diff-filter=ACMRD
  repeat_command git discovery-untracked 'validate untracked discovery' git git ls-files --others --exclude-standard -z
}

scenario_paths() {
  cat <<'EOF'
none|
docs|docs/architecture.md
shell|scripts/README.md
host-rust|mister/tools/host/src/main.rs
app-rust|apps/mister/src/ui_runner.rs
launcher-slint|apps/mister/ui/launcher.slint
catalog-rust|crates/catalog/src/arcade_catalog.rs
root-config|Cargo.toml
EOF
}

run_validation_groups() {
  local name path idx plan_file n
  while IFS='|' read -r name path; do
    [ -z "$scenario_filter" ] || [ "$name" = "$scenario_filter" ] || continue
    idx="$out/index-$name"
    rm -f "$idx"
    GIT_INDEX_FILE="$idx" git read-tree HEAD
    [ -z "$path" ] || synthetic_index "$path" "$idx"
    plan_file="$out/plan-$name.txt"
    GIT_INDEX_FILE="$idx" scripts/validate affected --print-plan >"$plan_file"
    if [ "$group" = validate ]; then
      repeat_command validate "$name" 'scripts/validate affected' none env GIT_INDEX_FILE="$idx" scripts/validate affected --json
    fi
    if [ "$group" = precommit ] || [ "$group" = all ]; then
      repeat_command precommit "$name" '.githooks/pre-commit' none env GIT_INDEX_FILE="$idx" .githooks/pre-commit
    fi
  done < <(scenario_paths)
}

state_path() {
  case "$1" in
    noop-warm) echo "" ;;
    touch-rust-bin) echo apps/mister/src/ui_runner.rs ;;
    touch-rust-core) echo crates/magik-core/src/input_state.rs ;;
    touch-slint-launcher) echo apps/mister/ui/launcher.slint ;;
    touch-slint-shared) echo apps/mister/ui/mister_window.slint ;;
    touch-build-rs) echo apps/mister/build.rs ;;
  esac
}

run_build_case() {
  local scenario="$1" state="$2" command="$3" path stamp n command_samples args=()
  command_samples="$(samples_for_group build)"
  path="$(state_path "$state")"
  if [ -n "$path" ]; then
    stamp="$out/mtime-$(safe_name "$state")"
    touch -r "$path" "$stamp"
    restore_files+=("$path"); restore_stamps+=("$stamp")
  fi
  for ((n=1; n<=warmups; n++)); do
    args=()
    if [ "$cold" -eq 1 ]; then
      rm -rf "$out/cold-target" "$out/cold-mirror"
      args+=(env MISTER_APPLE_CONTAINER_TARGET_DIR="$out/cold-target" MISTER_APPLE_CONTAINER_MIRROR_TARGET_DIR="$out/cold-mirror")
    fi
    args+=(scripts/bench-debug-build.sh --scenario "$scenario" --state "$state" --samples 1 --warmups 0 --label "$label-$scenario-$state-warmup$n")
    record_command build "$scenario-$state" "$command" "$n" 1 "" 0 "${args[@]}"
  done
  for ((n=1; n<=command_samples; n++)); do
    args=()
    if [ "$cold" -eq 1 ]; then
      rm -rf "$out/cold-target" "$out/cold-mirror"
      args+=(env MISTER_APPLE_CONTAINER_TARGET_DIR="$out/cold-target" MISTER_APPLE_CONTAINER_MIRROR_TARGET_DIR="$out/cold-mirror")
    fi
    args+=(scripts/bench-debug-build.sh --scenario "$scenario" --state "$state" --samples 1 --warmups 0 --label "$label-$scenario-$state-sample$n")
    record_command build "$scenario-$state" "$command" "$n" 0 "" 0 "${args[@]}"
  done
  if [ -n "$path" ]; then touch -r "$stamp" "$path"; fi
}

run_build_group() {
  local state
  for state in noop-warm touch-rust-bin touch-rust-core touch-slint-launcher touch-slint-shared touch-build-rs; do
    run_build_case build-ui-fast "$state" 'apps/mister/build-arm.sh --fast'
    run_build_case build-ui-device "$state" 'apps/mister/build-arm.sh --device'
  done
}

run_deploy_group() {
  local fast_bin="$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release/mister-magik-fb"
  local device_bin="$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
  if [ -f "$fast_bin" ]; then
    repeat_command deploy fake-fast 'local fake deploy --fast' none bash -c '
      set -euo pipefail
      bin="$1"; dest="$2"
      echo phase=receipt-hash
      before="$(shasum -a 256 "$bin" | awk "{print \$1}")"
      echo phase=manifest-preflight simulated=1
      echo phase=transfer simulated=1
      cp "$bin" "$dest"
      echo phase=remote-hash simulated=1
      after="$(shasum -a 256 "$dest" | awk "{print \$1}")"
      [ "$before" = "$after" ]
      echo phase=manifest-rebind simulated=1
      echo phase=final-verification simulated=1
    ' _ "$fast_bin" "$out/fake-fast.bin"
  else
    echo "SKIP deploy fake-fast: build $fast_bin first" | tee "$logs/deploy-fake-fast-skip.log"
  fi
  if [ -f "$device_bin" ]; then
    repeat_command deploy fake-device 'local fake deploy --device' none bash -c '
      set -euo pipefail
      bin="$1"; dest="$2"
      echo phase=receipt-hash
      before="$(shasum -a 256 "$bin" | awk "{print \$1}")"
      echo phase=manifest-preflight simulated=1
      echo phase=transfer simulated=1
      cp "$bin" "$dest"
      echo phase=remote-hash simulated=1
      after="$(shasum -a 256 "$dest" | awk "{print \$1}")"
      [ "$before" = "$after" ]
      echo phase=manifest-rebind simulated=1
      echo phase=final-verification simulated=1
    ' _ "$device_bin" "$out/fake-device.bin"
  else
    echo "SKIP deploy fake-device: build $device_bin first" | tee "$logs/deploy-fake-device-skip.log"
  fi
  if [ "$device" -eq 1 ]; then
    if ! device_command hil-unchanged 'scripts/deploy-rust.sh --fast' scripts/deploy-rust.sh --fast; then
      echo "MiSTer deployment failed; stopping HIL phase after the first wrapper failure" >&2
      return
    fi
    local rebuild_path="$ROOT/apps/mister/src/ui_runner.rs" rebuild_stamp="$out/mtime-hil-rebuild"
    touch -r "$rebuild_path" "$rebuild_stamp"
    restore_files+=("$rebuild_path"); restore_stamps+=("$rebuild_stamp")
    touch "$rebuild_path"
    if ! device_command hil-rebuilt 'touch app Rust + scripts/deploy-rust.sh --fast' scripts/deploy-rust.sh --fast; then
      touch -r "$rebuild_stamp" "$rebuild_path"
      echo "MiSTer rebuilt deployment failed; stopping HIL phase after the first wrapper failure" >&2
      return
    fi
    touch -r "$rebuild_stamp" "$rebuild_path"
    device_command hil-restart-only 'scripts/run-rust.sh launcher 0' scripts/run-rust.sh launcher 0 || {
      echo "MiSTer restart failed; stopping HIL phase after the first wrapper failure" >&2
      return
    }
  fi
}

if [ "${MISTER_LOCAL_WORKFLOW_INTERRUPT_SELF_TEST:-0}" = 1 ]; then
  record_command self-test interrupt 'interrupt cleanup fixture' 1 0 "" 0 \
    bash -c 'trap "" TERM; while :; do sleep 1; done' mister-local-workflow-interrupt-child
  exit 1
fi

if [ "${MISTER_LOCAL_WORKFLOW_EXPECTED_SELF_TEST:-0}" = 1 ]; then
  record_command self-test expected-nonzero 'expected status fixture' 1 0 "" 7 \
    bash -c 'exit 7'
  awk -F '\t' 'NR == 2 { exit !($6 == 7 && $7 == 7 && $8 == 1) }' "$results"
  exit
fi

case "$group" in
  git) run_git_group ;;
  precommit|validate) run_validation_groups ;;
  build) run_build_group ;;
  deploy) run_deploy_group ;;
  all) run_git_group; run_validation_groups; run_build_group; run_deploy_group ;;
esac

python3 - "$results" "$out" <<'PY'
import csv, json, math, statistics, sys
from collections import defaultdict
from pathlib import Path

results, out = Path(sys.argv[1]), Path(sys.argv[2])
rows = list(csv.DictReader(results.open(), delimiter="\t"))
measured = [r for r in rows if r["warmup"] == "0"]
groups = defaultdict(list)
for row in measured:
    groups[(row["group"], row["scenario"], row["command"])].append(row)

def percentile(values, fraction):
    values = sorted(values)
    if not values: return 0
    return values[max(0, math.ceil(len(values) * fraction) - 1)]

summary = []
for (group, scenario, command), members in groups.items():
    walls = [int(r["wall_ms"]) for r in members]
    summary.append({
        "group": group, "scenario": scenario, "command": command,
        "samples": len(walls), "median_ms": statistics.median(walls),
        "p90_ms": percentile(walls, .90), "min_ms": min(walls), "max_ms": max(walls),
        "median_git_trace_ms": statistics.median(int(r["git_trace_ms"]) for r in members),
        "median_harness_overhead_ms": statistics.median(int(r["harness_overhead_ms"]) for r in members),
        "stdev_ms": statistics.pstdev(walls),
        "failures": sum(r["status_ok"] != "1" for r in members),
        "compiled": sum(int(r["compiled"]) for r in members),
        "checked": sum(int(r["checked"]) for r in members),
        "cargo_runs": sum(int(r["cargo_runs"]) for r in members),
        "container_runs": sum(int(r["container_runs"]) for r in members),
        "validation_checks": sum(int(r["validation_checks"]) for r in members),
    })
summary.sort(key=lambda x: x["median_ms"], reverse=True)
check_costs = defaultdict(list)
for row in measured:
    if row["group"] not in ("validate", "precommit"):
        continue
    log = Path(row["log_path"])
    sources = [log, log.with_suffix(".time")]
    for source in sources:
        if not source.is_file(): continue
        for line in source.read_text(errors="replace").splitlines():
            if "VALIDATION " not in line or "check=" not in line or "duration_ms=" not in line: continue
            fields = dict(part.split("=", 1) for part in line.split() if "=" in part)
            if "check" in fields and "duration_ms" in fields:
                check_costs[fields["check"]].append(int(fields["duration_ms"]))

ratios = []
build_medians = {(x["scenario"], x["group"]): x["median_ms"] for x in summary if x["group"] == "build"}
for (scenario, _), median in build_medians.items():
    if not scenario.endswith("-noop-warm"): continue
    prefix = scenario.removesuffix("-noop-warm")
    for (candidate, _), edited in build_medians.items():
        if candidate.startswith(prefix + "-") and candidate != scenario and median:
            ratios.append({"baseline":scenario,"scenario":candidate,"warm_edited_ratio":edited/median})

analysis = {
    "schema":"mister-magik-local-workflow-v2",
    "scenarios":summary,
    "validation_check_costs": [
        {"check": name, "samples": len(values), "median_ms": statistics.median(values),
         "p90_ms": percentile(values, .90), "total_ms": sum(values)}
        for name, values in sorted(check_costs.items(), key=lambda item: sum(item[1]), reverse=True)
    ],
    "build_ratios": ratios,
}
(out / "summary.json").write_text(json.dumps(analysis, indent=2) + "\n")
with (out / "summary.tsv").open("w", newline="") as f:
    fields = list(summary[0]) if summary else ["group","scenario"]
    writer = csv.DictWriter(f, fields, delimiter="\t")
    writer.writeheader(); writer.writerows(summary)

print("\nRanked local workflow costs")
print("median   p90      group       scenario")
for item in summary:
    print(f'{item["median_ms"]/1000:7.3f}s {item["p90_ms"]/1000:7.3f}s  {item["group"]:10}  {item["scenario"]}')

plans = {}
for path in out.glob("plan-*.txt"):
    plans[path.stem.removeprefix("plan-")] = [line.strip() for line in path.read_text().splitlines() if line.strip()]
if plans:
    (out / "validation-plans.json").write_text(json.dumps(plans, indent=2) + "\n")
    print("\nValidation routing")
    for name, checks in plans.items():
        unconditional = [x for x in ("format","validation-routing","host-tools-fast") if x in checks]
        print(f'{name:16} checks={len(checks):2} fixed={",".join(unconditional) or "none"}')

if check_costs:
    print("\nValidation checks ranked by cumulative measured cost")
    for name, values in sorted(check_costs.items(), key=lambda item: sum(item[1]), reverse=True):
        print(f'{sum(values)/1000:8.3f}s total  {statistics.median(values)/1000:7.3f}s median  {name}')

if ratios:
    print("\nWarm no-op to representative edit ratios")
    for item in sorted(ratios, key=lambda x: x["warm_edited_ratio"], reverse=True):
        print(f'{item["warm_edited_ratio"]:7.2f}x  {item["scenario"]} vs {item["baseline"]}')

duplicates = [x for x in summary if x["group"] in ("build", "deploy") and (x["cargo_runs"] > x["samples"] or x["container_runs"] > x["samples"])]
if duplicates:
    print("\nPotential repeated build work")
    for x in duplicates:
        print(f'{x["group"]}/{x["scenario"]}: cargo={x["cargo_runs"]} container={x["container_runs"]} samples={x["samples"]}')
PY

printf '\nResults: %s\n' "$out"
