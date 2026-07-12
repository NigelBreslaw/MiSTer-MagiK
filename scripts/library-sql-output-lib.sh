#!/usr/bin/env bash
# Helpers for parsing `scripts/mister db` output.

library_sql_first_result_line() {
  awk '
    NF == 0 { next }
    /\r$/ { sub(/\r$/, "") }
    /^library_sql_timing_tsv([[:space:]]|$)/ { next }
    /^library_sql_result_tsv([[:space:]]|$)/ { next }
    /^library_sql_batch_tsv([[:space:]]|$)/ { next }
    /^library_sql_transport_tsv([[:space:]]|$)/ { next }
    header_seen == 0 { header_seen = 1; next }
    { print; exit }
  '
}

library_sql_first_result_number() {
  library_sql_first_result_line | awk '
    NF {
      value = $NF
      gsub(/[^0-9]/, "", value)
      print value
      exit
    }
  '
}

library_sql_output_self_test() {
  local fixture line number
  fixture=$'count(*)\n1\nlibrary_sql_timing_tsv\t/media/fat/mister-magik/library.sqlite3\t123\tdeadbeef\t1\t2\t3\t4\t5\t6\t7\t8\tok\n'
  number="$(printf '%s' "$fixture" | library_sql_first_result_number)"
  if [ "$number" != "1" ]; then
    echo "library SQL number parser failed: $number" >&2
    return 1
  fi
  fixture=$'launch_ref\n/media/fat/_Arcade/1941.mra\nlibrary_sql_timing_tsv\t/media/fat/mister-magik/library.sqlite3\t123\tdeadbeef\t1\t2\t3\t4\t5\t6\t7\t8\tok\n'
  line="$(printf '%s' "$fixture" | library_sql_first_result_line)"
  if [ "$line" != "/media/fat/_Arcade/1941.mra" ]; then
    echo "library SQL line parser failed: $line" >&2
    return 1
  fi
  fixture=$'name\tcount\nlauncher_catalog\t2240\nlibrary_sql_timing_tsv\t/media/fat/mister-magik/library.sqlite3\t123\tdeadbeef\t1\t2\t3\t4\t5\t6\t7\t8\tok\n'
  number="$(printf '%s' "$fixture" | library_sql_first_result_number)"
  if [ "$number" != "2240" ]; then
    echo "library SQL tabular parser failed: $number" >&2
    return 1
  fi
  fixture=$'library_sql_result_tsv\t1\tbegin\tdeadbeef\ncount(*)\n7\nlibrary_sql_timing_tsv\t/path\t1\tdeadbeef\t1\t\t1\t1\t1\t1\t1\t1\t1\t1\nlibrary_sql_result_tsv\t1\tend\tdeadbeef\nlibrary_sql_batch_tsv\t1\t1\t2\n'
  number="$(printf '%s' "$fixture" | library_sql_first_result_number)"
  if [ "$number" != "7" ]; then
    echo "library SQL batch marker parser failed: $number" >&2
    return 1
  fi
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -euo pipefail
  library_sql_output_self_test
  echo "library-sql-output-lib self-test ok"
fi
