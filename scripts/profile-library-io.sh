#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Profile CPU and storage activity during one library scan/import benchmark.
set -euo pipefail

echo "ERROR: profile-library-io targeted the retired V2 monolith; use V3 shard/rebuild profiles" >&2
exit 2

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/lib/magik-layout.sh"
magik_layout_select dev
REMOTE="$MISTER_MAGIK_BIN"
REMOTE_DIR="$MISTER_MAGIK_APP_DIR"
DEPLOY_LOCK="$REMOTE_DIR/deploy.lock"
BENCH_DIR="$HERE/history/toolchain-bench"
OUT_DIR="$HERE/build/library-io-profiles"
TSV="$BENCH_DIR/results-library-io.tsv"
LABEL=""
REPLACE_LABEL=0
SQLITE_PATH="$MISTER_MAGIK_APP_DIR/library-io-bench.sqlite3"
SQLITE_BUILD_DIR=""
SAMPLE_LIMIT=180
source "$HERE/scripts/lib/bench-context-lib.sh"
source "$HERE/scripts/lib/benchmark-cleanup-lib.sh"

library_io_pid_identity_valid() {
  local executable="$1" command_line="$2"
  [[ "${executable##*/}" == "mister-magik-fb" && "$command_line" == *"mister-magik-fb library-refresh"* ]]
}

library_io_lock_owner_matches() {
  local expected="$1" actual="$2"
  [[ -n "$expected" && "$expected" == "$actual" ]]
}

library_io_mark_lock_attempt() {
  deploy_lock_active=1
}

library_io_self_test() {
  library_io_pid_identity_valid "/media/fat/mister-magik-dev/mister-magik-fb" "/media/fat/mister-magik-dev/mister-magik-fb library-refresh"
  if library_io_pid_identity_valid "/bin/sh" "sh -c mister-magik-fb library-refresh"; then
    echo "library I/O PID identity accepted a shell wrapper" >&2
    return 1
  fi
  if library_io_pid_identity_valid "/media/fat/mister-magik-dev/mister-magik-fb" "/media/fat/mister-magik-dev/mister-magik-fb ui launcher 0"; then
    echo "library I/O PID identity accepted the wrong subcommand" >&2
    return 1
  fi
  library_io_lock_owner_matches "run-token" "run-token"
  if library_io_lock_owner_matches "run-token" "other-token"; then
    echo "library I/O deploy lock accepted a foreign owner" >&2
    return 1
  fi
  if library_io_lock_owner_matches "" ""; then
    echo "library I/O deploy lock accepted an empty owner" >&2
    return 1
  fi
  deploy_lock_active=0
  library_io_mark_lock_attempt
  if [[ "$deploy_lock_active" != "1" ]]; then
    echo "library I/O deploy lock attempt was not cleanup-owned before acknowledgement" >&2
    return 1
  fi
  echo "profile-library-io self-test ok"
}

usage() {
  cat <<'EOF'
Usage: scripts/profile-library-io.sh LABEL [--replace-label] [--sqlite-build-dir DIR] [--sqlite-path PATH] [--sample-limit N] [--self-test]

Runs one production library-refresh pass while sampling process CPU, process I/O,
system CPU/iowait, and backing-disk counters once per second.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --self-test) library_io_self_test; exit 0 ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --sqlite-build-dir) SQLITE_BUILD_DIR="${2:?}"; shift 2 ;;
    --sqlite-path) SQLITE_PATH="${2:?}"; shift 2 ;;
    --sample-limit) SAMPLE_LIMIT="${2:?}"; shift 2 ;;
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
  LABEL="library-io-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$SAMPLE_LIMIT" =~ ^[0-9]+$ ]]; then
  echo "--sample-limit must be an integer" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR" "$OUT_DIR"
if [[ ! -f "$TSV" ]]; then
  echo "label	type	elapsed_s	field_a	field_b	field_c	field_d	field_e	field_f	field_g	field_h	field_i	field_j	notes" >"$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

remote_run() {
  "$MISTER" run "$1"
}

magik_command() {
  remote_run "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiKDev >/dev/null 2>&1; then printf '$1\n' > /dev/MiSTer_cmd; else echo 'MiSTer_MagiKDev supervision unavailable' >&2; exit 12; fi" >/dev/null
}

launcher_suspended=0
run_with_launcher_suspended() {
  magik_command "mister_magik_suspend"
  launcher_suspended=1
  set +e
  remote_run "$1"
  local status=$?
  set -e
  magik_command "mister_magik_resume"
  launcher_suspended=0
  return "$status"
}

remote_script="$(mktemp)"
cat >"$remote_script" <<'REMOTE_SCRIPT'
#!/bin/sh
set -eu
label="$1"
remote="$2"
sqlite_path="$3"
sqlite_build_dir="$4"
sample_limit="$5"
log="/tmp/${label}-library-io.log"

disk_name_for_path() {
  mount_src="$(df -P "$(dirname "$1")" 2>/dev/null | awk 'NR==2 { print $1 }')"
  base="$(basename "$mount_src")"
  case "$base" in
    mmcblk*p[0-9]*) echo "$base" | sed 's/p[0-9][0-9]*$//' ;;
    sd*[0-9]*) echo "$base" | sed 's/[0-9][0-9]*$//' ;;
    root|overlay|fuseblk|*) awk '$3 ~ /^mmcblk[0-9]+$/ { print $3; found=1; exit } END { if (!found) print "'$base'" }' /proc/diskstats ;;
  esac
}

read_diskstats() {
  disk="$1"
  awk -v disk="$disk" '$3 == disk { print $4, $8, $13; found=1 } END { if (!found) print "0 0 0" }' /proc/diskstats
}

read_proc_io() {
  pid="$1"
  awk '
    $1 == "read_bytes:" { rb = $2 }
    $1 == "write_bytes:" { wb = $2 }
    END { printf "%s %s\n", rb + 0, wb + 0 }
  ' "/proc/$pid/io" 2>/dev/null || echo "0 0"
}

disk="$(disk_name_for_path "$sqlite_path")"
rm -f "$sqlite_path" "$log"
if [ -n "$sqlite_build_dir" ]; then
  env MISTER_LIBRARY_BENCH_LABEL="$label" \
    MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=1 \
    MISTER_LIBRARY_SQLITE="$sqlite_path" \
    MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH=1 \
    MISTER_LIBRARY_SQLITE_BUILD_DIR="$sqlite_build_dir" \
    "$remote" library-refresh >"$log" 2>&1 &
else
  env MISTER_LIBRARY_BENCH_LABEL="$label" \
    MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=1 \
    MISTER_LIBRARY_SQLITE="$sqlite_path" \
    MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH=1 \
    "$remote" library-refresh >"$log" 2>&1 &
fi
pid=$!
attempts=0
executable=""
command_line=""
while [ "$attempts" -lt 20 ] && kill -0 "$pid" 2>/dev/null; do
  executable="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
  command_line="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
  if [ "$(basename "$executable")" = "mister-magik-fb" ] && printf '%s\n' "$command_line" | grep -Fq "mister-magik-fb library-refresh"; then
    break
  fi
  sleep 0.05
  attempts=$((attempts + 1))
done
if [ "$(basename "$executable")" != "mister-magik-fb" ] || ! printf '%s\n' "$command_line" | grep -Fq "mister-magik-fb library-refresh"; then
  echo "library_io_error_tsv\t$label\tpid=$pid\treason=unexpected-command" >&2
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  exit 1
fi
echo "library_io_process_tsv\t$label\tpid=$pid\texecutable=$executable\tcommand=$command_line\tvalid=1"
start="$(date +%s)"
i=0
while kill -0 "$pid" 2>/dev/null && [ "$i" -lt "$sample_limit" ]; do
  now="$(date +%s)"
  elapsed=$((now - start))
  proc_ticks="$(awk '{ print $14, $15 }' "/proc/$pid/stat" 2>/dev/null || echo "0 0")"
  proc_io="$(read_proc_io "$pid")"
  cpu="$(awk '/^cpu / { print $2, $3, $4, $5, $6, $7, $8 }' /proc/stat)"
  diskstats="$(read_diskstats "$disk")"
  echo "library_io_sample_tsv	$label	$elapsed	$proc_ticks	$proc_io	$cpu	$diskstats	disk=$disk sqlite=$sqlite_path build_dir=${sqlite_build_dir:-none}"
  sleep 1
  i=$((i + 1))
done
set +e
wait "$pid"
rc=$?
set -e
cat "$log"
echo "library_io_done_tsv	$label	$(( $(date +%s) - start ))	rc=$rc disk=$disk sqlite=$sqlite_path build_dir=${sqlite_build_dir:-none}"
exit "$rc"
REMOTE_SCRIPT
chmod +x "$remote_script"

remote_profile="/tmp/profile-library-io-${LABEL}.sh"
cleanup_report="$OUT_DIR/${LABEL}-cleanup.txt"
report="$OUT_DIR/${LABEL}.report.tsv"
deploy_lock_active=0
deploy_lock_token="library-io-${LABEL}-$$-$(date -u +%s)"
remote_profile_uploaded=0
profile_library_io_cleanup_complete=0
profile_library_io_cleanup_status=0

library_io_release_owned_lock() {
  [[ "$deploy_lock_active" == "1" ]] || return 0
  if remote_run "if [ ! -f '$DEPLOY_LOCK' ]; then echo 'deploy lock missing before owned release' >&2; exit 24; fi; owner=\$(cat '$DEPLOY_LOCK' 2>/dev/null || true); if [ \"\$owner\" != '$deploy_lock_token' ]; then echo \"deploy lock owned by another runner: \$owner\" >&2; exit 25; fi; rm -f '$DEPLOY_LOCK'" >/dev/null; then
    deploy_lock_active=0
    return 0
  fi
  return 1
}

profile_library_io_cleanup() {
  local cleanup_status=0
  if [[ "$profile_library_io_cleanup_complete" == "1" ]]; then
    return "$profile_library_io_cleanup_status"
  fi
  rm -f "$remote_script"
  if ! library_io_release_owned_lock; then
    cleanup_status=1
  fi
  remote_run "rm -f '$remote_profile'" >/dev/null 2>&1 || cleanup_status=1
  remote_profile_uploaded=0
  if [[ "$launcher_suspended" == "1" ]]; then
    magik_command "mister_magik_resume" >/dev/null 2>&1 || cleanup_status=1
    launcher_suspended=0
  fi
  if benchmark_cleanup_assert_no_arming_files "$MISTER" "$cleanup_report"; then
    printf 'cleanup_tsv\tlabel=%s\tvalid=1\tinvalid_reason=ok\n' "$LABEL" >>"$report"
  else
    printf 'cleanup_tsv\tlabel=%s\tvalid=0\tinvalid_reason=stale-arming-or-device-error\n' "$LABEL" >>"$report"
    cleanup_status=1
  fi
  profile_library_io_cleanup_status="$cleanup_status"
  profile_library_io_cleanup_complete=1
  return "$profile_library_io_cleanup_status"
}
benchmark_cleanup_install profile_library_io_cleanup

binary_path="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
deployed_sha256="$(bench_context_remote_sha256 "$MISTER" "$REMOTE" || true)"
deployed_sha256="${deployed_sha256:-missing}"
if ! bench_context_require_binary_contract "$binary_path" "$deployed_sha256" ui release-device launcher; then
  echo "library I/O binary identity verification failed local=$(bench_context_sha256_file "$binary_path") deployed=$deployed_sha256 features=$(bench_context_binary_features "$binary_path") expected_features=ui" >&2
  exit 1
fi
binary_fields="$(bench_context_binary_fields release-device launcher ui "$binary_path" production verified "$deployed_sha256")"
source_fields="$(bench_context_source_fields "$HERE")"
printf 'run_context_tsv\tlabel=%s\tcommand=profile-library-io\t%s\t%s\n' "$LABEL" "$source_fields" "$binary_fields" | tee "$report"

library_io_mark_lock_attempt
remote_run "mkdir -p '$REMOTE_DIR'; if (set -C; umask 077; printf '%s\\n' '$deploy_lock_token' > '$DEPLOY_LOCK') 2>/dev/null; then :; else echo 'deploy lock already exists' >&2; exit 23; fi"
magik_command "mister_magik_suspend"
launcher_suspended=1
"$MISTER" put "$remote_script" "$remote_profile" >/dev/null
remote_profile_uploaded=1
remote_run "chmod +x '$REMOTE' '$remote_profile'"
library_io_release_owned_lock
magik_command "mister_magik_resume"
launcher_suspended=0

echo "== library I/O profile label=$LABEL sqlite=$SQLITE_PATH build_dir=${SQLITE_BUILD_DIR:-none}"
set +e
OUT="$(run_with_launcher_suspended "'$remote_profile' '$LABEL' '$REMOTE' '$SQLITE_PATH' '$SQLITE_BUILD_DIR' '$SAMPLE_LIMIT'" 2>&1)"
run_status=$?
set -e
printf '%s\n' "$OUT" | tee -a "$report"

set +e
profile_library_io_cleanup
cleanup_status=$?
set -e
if [[ "$run_status" -ne 0 ]]; then
  printf 'validity_tsv\tlabel=%s\tvalid=0\tinvalid_reason=benchmark-failed\trun_status=%s\tcleanup_status=%s\n' \
    "$LABEL" "$run_status" "$cleanup_status" | tee -a "$report"
  exit "$run_status"
fi
if [[ "$cleanup_status" -ne 0 ]]; then
  printf 'validity_tsv\tlabel=%s\tvalid=0\tinvalid_reason=cleanup-failed\trun_status=%s\tcleanup_status=%s\n' \
    "$LABEL" "$run_status" "$cleanup_status" | tee -a "$report"
  exit "$cleanup_status"
fi

echo "$OUT" | awk -F '\t' -v label="$LABEL" '
  BEGIN { OFS = "\t" }
  $1 == "library_io_sample_tsv" {
    n = split($4, proc, " ")
    split($5, io, " ")
    split($6, cpu, " ")
    split($7, disk, " ")
    print label, "sample", $3, proc[1], proc[2], io[1], io[2], cpu[1], cpu[3], cpu[4], cpu[5], disk[1], disk[2], "disk_io_ms=" disk[3] " " $8
  }
  $1 == "library_scan_bench_tsv" {
    print label, $4, $3, $5, "", "", "", "", "", "", "", "", "", $6
  }
  $1 == "library_scan_timing" {
    print label, "scan_stage_" $2, int(($3 + 500) / 1000), "", "", "", "", "", "", "", "", "", "", $4
  }
  $1 == "library_import_timing" {
    print label, "import_stage_" $2, int(($3 + 500) / 1000), "", "", "", "", "", "", "", "", "", "", $4
  }
  $1 == "library_sqlite_publish_tsv" {
    print label, "sqlite_publish_" $4, int(($11 + 500) / 1000), "", "", "", "", "", "", "", "", "", "", "bytes=" $5 " copy_ms=" $7 " build_sync_ms=" $6 " final_sync_ms=" $8 " rename_ms=" $9 " parent_sync_ms=" $10 " progress_events=" $12 " result=" $13
  }
  $1 == "library_refresh" && $2 == "done" {
    n = split($3, fields, " ")
    us = ""
    for (i = 1; i <= n; i++) {
      if (fields[i] ~ /^scan_us=/) {
        us = fields[i]
        sub(/^scan_us=/, "", us)
      }
    }
    print label, "refresh_done", 0, "", "", "", "", "", "", "", "", "", "", $3
  }
  $1 == "library_io_done_tsv" {
    print label, "done", $3, "", "", "", "", "", "", "", "", "", "", $4
  }
' >>"$TSV"

printf 'validity_tsv\tlabel=%s\tvalid=1\tinvalid_reason=ok\trun_status=0\tcleanup_status=0\n' "$LABEL" | tee -a "$report"
echo "appended to $TSV"
