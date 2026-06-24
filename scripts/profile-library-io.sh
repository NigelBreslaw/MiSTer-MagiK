#!/usr/bin/env bash
# Profile CPU and storage activity during one library scan/import benchmark.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
REMOTE_DIR="/media/fat/mister-magik"
DEPLOY_LOCK="$REMOTE_DIR/deploy.lock"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-library-io.tsv"
LABEL=""
REPLACE_LABEL=0
SQLITE_PATH="/media/fat/mister-magik/library-io-bench.sqlite3"
SQLITE_BUILD_DIR=""
SAMPLE_LIMIT=180

usage() {
  cat <<'EOF'
Usage: scripts/profile-library-io.sh LABEL [--replace-label] [--sqlite-build-dir DIR] [--sqlite-path PATH] [--sample-limit N]

Runs one production library-refresh pass while sampling process CPU, process I/O,
system CPU/iowait, and backing-disk counters once per second.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
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

mkdir -p "$BENCH_DIR"
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
  remote_run "if [ -p /dev/MiSTer_cmd ] && pidof MiSTer_MagiK >/dev/null 2>&1; then printf '$1\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}

cleanup_deploy_lock() {
  remote_run "rm -f '$DEPLOY_LOCK'" >/dev/null 2>&1 || true
  magik_command "mister_magik_resume"
}

run_with_launcher_suspended() {
  trap 'magik_command "mister_magik_resume"' RETURN
  magik_command "mister_magik_suspend"
  remote_run "$1"
  magik_command "mister_magik_resume"
  trap - RETURN
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
env="MISTER_LIBRARY_BENCH_LABEL=$label MISTER_LIBRARY_BENCH_ACTIVE_ITERATION=1 MISTER_LIBRARY_SQLITE=$sqlite_path MISTER_MAGIK_FOREGROUND_LIBRARY_REFRESH=1"
if [ -n "$sqlite_build_dir" ]; then
  env="$env MISTER_LIBRARY_SQLITE_BUILD_DIR=$sqlite_build_dir"
fi
sh -c "$env $remote library-refresh" >"$log" 2>&1 &
pid=$!
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
trap 'rm -f "$remote_script"; "$MISTER" run "rm -f '"$remote_profile"'" >/dev/null 2>&1 || true' EXIT

trap cleanup_deploy_lock EXIT
remote_run "mkdir -p '$REMOTE_DIR'; : > '$DEPLOY_LOCK'"
magik_command "mister_magik_suspend"
"$MISTER" put "$remote_script" "$remote_profile" >/dev/null
remote_run "chmod +x '$REMOTE' '$remote_profile'; rm -f '$DEPLOY_LOCK'"
magik_command "mister_magik_resume"
trap - EXIT

echo "== library I/O profile label=$LABEL sqlite=$SQLITE_PATH build_dir=${SQLITE_BUILD_DIR:-none}"
OUT=$(run_with_launcher_suspended "'$remote_profile' '$LABEL' '$REMOTE' '$SQLITE_PATH' '$SQLITE_BUILD_DIR' '$SAMPLE_LIMIT'" 2>&1) || true
echo "$OUT"

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

echo "appended to $TSV"
