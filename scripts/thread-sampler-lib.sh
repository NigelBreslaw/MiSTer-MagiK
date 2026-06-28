#!/usr/bin/env bash
# Shared real-device per-thread sampler for benchmark scripts.

thread_sample_enabled="${thread_sample_enabled:-0}"
thread_sample_pid_file=""
thread_sample_remote_tsv=""
thread_sample_local_tsv=""
thread_sample_remote_log=""
thread_sample_local_log=""
thread_sample_remote_script=""
thread_sample_local_script=""

thread_sample_shell_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

thread_sample_write_remote_script() {
  local local_script="$1"
  cat >"$local_script" <<'REMOTE_SAMPLER'
#!/bin/sh
duration_secs="$1"
process_name="$2"
out="$3"
interval_secs="${4:-1}"

if [ -z "$duration_secs" ] || [ -z "$process_name" ] || [ -z "$out" ]; then
  echo "usage: thread-sampler DURATION PROCESS_NAME OUT [INTERVAL]" >&2
  exit 2
fi

sample=0
end_epoch=$(( $(date +%s) + duration_secs ))

printf 'thread_sample_tsv\tsample\tts_unix\tpid\ttid\tthread_name\tstate\tprocessor\tutime_jiffies\tstime_jiffies\tutime_delta_jiffies\tstime_delta_jiffies\tvoluntary_ctxt_switches\tnonvoluntary_ctxt_switches\tvoluntary_delta\tnonvoluntary_delta\tvmrss_kb\tvmhwm_kb\tsched_exec_runtime_ms\tsched_nr_switches\tsched_wait_sum_ms\n' >"$out"

while [ "$(date +%s)" -le "$end_epoch" ]; do
  pids="$(pidof "$process_name" 2>/dev/null || true)"
  set -- $pids
  pid="${1:-}"
  ts="$(date +%s)"

  if [ -n "$pid" ] && [ -d "/proc/$pid/task" ]; then
    sched_supported=0
    [ -r "/proc/$pid/task/$pid/sched" ] && sched_supported=1
    for task_dir in /proc/"$pid"/task/*; do
      [ -d "$task_dir" ] || continue
      tid="${task_dir##*/}"
      stat_line=""
      read -r stat_line <"$task_dir/stat" 2>/dev/null || true
      [ -n "$stat_line" ] || continue

      thread_name="${stat_line#*(}"
      thread_name="${thread_name%%)*}"
      stat_rest="${stat_line##*) }"
      set -- $stat_rest
      state="${1:-?}"
      utime="${12:-0}"
      stime="${13:-0}"
      processor="${37:--1}"

      vmrss=0
      vmhwm=0
      voluntary=0
      nonvoluntary=0
      if [ -r "$task_dir/status" ]; then
        while read -r key value _rest; do
          case "$key" in
            VmRSS:) vmrss="${value:-0}" ;;
            VmHWM:) vmhwm="${value:-0}" ;;
            voluntary_ctxt_switches:) voluntary="${value:-0}" ;;
            nonvoluntary_ctxt_switches:) nonvoluntary="${value:-0}" ;;
          esac
        done <"$task_dir/status"
      fi

      sched_exec=0
      sched_switches=0
      sched_wait=0
      if [ "$sched_supported" = "1" ] && [ -r "$task_dir/sched" ]; then
        while IFS=: read -r key value; do
          value="${value#"${value%%[! 	]*}"}"
          value="${value%"${value##*[! 	]}"}"
          case "$key" in
            *se.sum_exec_runtime*) sched_exec="${value:-0}" ;;
            *nr_switches*) sched_switches="${value:-0}" ;;
            *se.statistics.wait_sum*) sched_wait="${value:-0}" ;;
          esac
        done <"$task_dir/sched"
      fi

      utime_delta=0
      stime_delta=0
      voluntary_delta=0
      nonvoluntary_delta=0
      eval "seen_tid=\${thread_seen_$tid:-0}"
      if [ "$seen_tid" = "1" ]; then
        eval "prev_utime=\${thread_utime_$tid:-$utime}"
        eval "prev_stime=\${thread_stime_$tid:-$stime}"
        eval "prev_voluntary=\${thread_voluntary_$tid:-$voluntary}"
        eval "prev_nonvoluntary=\${thread_nonvoluntary_$tid:-$nonvoluntary}"
        utime_delta=$(( utime - prev_utime ))
        stime_delta=$(( stime - prev_stime ))
        voluntary_delta=$(( voluntary - prev_voluntary ))
        nonvoluntary_delta=$(( nonvoluntary - prev_nonvoluntary ))
      fi

      printf 'thread_sample_tsv\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$sample" "$ts" "$pid" "$tid" "$thread_name" "$state" "$processor" \
        "$utime" "$stime" "$utime_delta" "$stime_delta" "$voluntary" "$nonvoluntary" \
        "$voluntary_delta" "$nonvoluntary_delta" "$vmrss" "$vmhwm" \
        "$sched_exec" "$sched_switches" "$sched_wait" >>"$out"
      eval "thread_seen_$tid=1"
      eval "thread_utime_$tid=$utime"
      eval "thread_stime_$tid=$stime"
      eval "thread_voluntary_$tid=$voluntary"
      eval "thread_nonvoluntary_$tid=$nonvoluntary"
    done
  fi

  sample=$(( sample + 1 ))
  sleep "$interval_secs"
done
REMOTE_SAMPLER
}

thread_sample_start() {
  local label="$1" case_name="$2" out_dir="$3" duration_secs="$4"
  local process_name="${5:-mister-magik-fb}"
  if [[ "$thread_sample_enabled" != "1" ]]; then
    return 0
  fi
  if [[ -z "${MISTER:-}" ]]; then
    echo "thread sampler requires MISTER to be set" >&2
    return 2
  fi

  mkdir -p "$out_dir"
  thread_sample_remote_tsv="/tmp/${label}-${case_name}-thread-sample.tsv"
  thread_sample_local_tsv="$out_dir/${label}-${case_name}-thread-sample.tsv"
  thread_sample_remote_log="/tmp/${label}-${case_name}-thread-sample.log"
  thread_sample_local_log="$out_dir/${label}-${case_name}-thread-sample.log"
  thread_sample_remote_script="/tmp/${label}-${case_name}-thread-sampler.sh"
  thread_sample_pid_file="/tmp/${label}-${case_name}-thread-sampler.pid"
  thread_sample_local_script="$(mktemp "${TMPDIR:-/tmp}/mister-magik-thread-sampler.XXXXXX")"

  thread_sample_write_remote_script "$thread_sample_local_script"
  "$MISTER" put "$thread_sample_local_script" "$thread_sample_remote_script" >/dev/null
  rm -f "$thread_sample_local_script"
  local q_script q_tsv q_log q_pid q_process
  q_script="$(thread_sample_shell_quote "$thread_sample_remote_script")"
  q_tsv="$(thread_sample_shell_quote "$thread_sample_remote_tsv")"
  q_log="$(thread_sample_shell_quote "$thread_sample_remote_log")"
  q_pid="$(thread_sample_shell_quote "$thread_sample_pid_file")"
  q_process="$(thread_sample_shell_quote "$process_name")"
  "$MISTER" run "chmod +x $q_script; rm -f $q_tsv $q_log $q_pid; nice -n 19 sh $q_script '$duration_secs' $q_process $q_tsv >$q_log 2>&1 & echo \$! >$q_pid" >/dev/null
}

thread_sample_stop() {
  if [[ "$thread_sample_enabled" != "1" || -z "$thread_sample_pid_file" ]]; then
    return 0
  fi
  local q_pid q_script
  q_pid="$(thread_sample_shell_quote "$thread_sample_pid_file")"
  q_script="$(thread_sample_shell_quote "$thread_sample_remote_script")"
  "$MISTER" run "if [ -f $q_pid ]; then kill \$(cat $q_pid) 2>/dev/null || true; fi; rm -f $q_pid $q_script" >/dev/null 2>&1 || true
}

thread_sample_collect() {
  if [[ "$thread_sample_enabled" != "1" || -z "$thread_sample_remote_tsv" ]]; then
    return 0
  fi
  "$MISTER" get "$thread_sample_remote_tsv" "$thread_sample_local_tsv" >/dev/null 2>&1 || true
  "$MISTER" get "$thread_sample_remote_log" "$thread_sample_local_log" >/dev/null 2>&1 || true
}

thread_sample_emit_artifacts() {
  if [[ "$thread_sample_enabled" != "1" || -z "$thread_sample_local_tsv" ]]; then
    return 0
  fi
  if declare -F emit_artifact_row >/dev/null 2>&1; then
    emit_artifact_row "thread_sample_tsv" "$thread_sample_local_tsv" "$thread_sample_remote_tsv"
    emit_artifact_row "thread_sample_log" "$thread_sample_local_log" "$thread_sample_remote_log"
  else
    local exists="false" bytes="0" log_exists="false" log_bytes="0"
    if [[ -f "$thread_sample_local_tsv" ]]; then
      exists="true"
      bytes="$(wc -c <"$thread_sample_local_tsv" | tr -d ' ')"
    fi
    if [[ -f "$thread_sample_local_log" ]]; then
      log_exists="true"
      log_bytes="$(wc -c <"$thread_sample_local_log" | tr -d ' ')"
    fi
    printf 'artifact_tsv\tlabel=%s\tkind=thread_sample_tsv\tlocal_path=%s\tremote_path=%s\texists=%s\tbytes=%s\n' \
      "$label" "$thread_sample_local_tsv" "$thread_sample_remote_tsv" "$exists" "$bytes"
    printf 'artifact_tsv\tlabel=%s\tkind=thread_sample_log\tlocal_path=%s\tremote_path=%s\texists=%s\tbytes=%s\n' \
      "$label" "$thread_sample_local_log" "$thread_sample_remote_log" "$log_exists" "$log_bytes"
  fi
}

thread_sample_finish() {
  thread_sample_stop
  thread_sample_collect
  thread_sample_emit_artifacts
}
