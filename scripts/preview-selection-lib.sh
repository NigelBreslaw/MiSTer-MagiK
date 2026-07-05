#!/usr/bin/env bash
# Helpers for benchmark scripts that need to start on preview-bearing catalog rows.

preview_selection_sql_quote() {
  local value="$1"
  printf "%s" "$value" | sed "s/'/''/g"
}

preview_selection_index_query() {
  local system="$1"
  local quoted
  quoted="$(preview_selection_sql_quote "$system")"
  cat <<SQL
WITH system_rows AS (
  SELECT
    row_number() OVER (ORDER BY ordinal) - 1 AS selected_index,
    title,
    preview_asset_key,
    has_preview
  FROM launcher_catalog
  WHERE system_id = '$quoted'
)
SELECT selected_index, title, preview_asset_key
FROM system_rows
WHERE has_preview = 1
  AND preview_asset_key IS NOT NULL
  AND preview_asset_key != ''
ORDER BY selected_index
LIMIT 1;
SQL
}

preview_selection_parse_index() {
  awk -F '\t' '
    $1 ~ /^[0-9]+$/ {
      print $1
      found = 1
      exit
    }
    END { if (!found) exit 1 }
  '
}

preview_selection_index_for_system() {
  local mister="$1"
  local system="$2"
  if [[ ! "$system" =~ ^[A-Za-z0-9_.-]+$ ]]; then
    echo "system must contain only letters, numbers, _, ., or -: $system" >&2
    return 2
  fi
  local query
  query="$(preview_selection_index_query "$system")"
  "$mister" db "$query" | preview_selection_parse_index
}

preview_selection_self_test() {
  local parsed
  parsed="$(printf 'selected_index\ttitle\tpreview_asset_key\n6\t2020 Super Baseball\t2020bb\nQuery time: 1ms\n' | preview_selection_parse_index)"
  [[ "$parsed" == "6" ]] || {
    echo "preview selection parser did not extract numeric row" >&2
    return 1
  }

  if printf 'selected_index\ttitle\tpreview_asset_key\nQuery time: 1ms\n' | preview_selection_parse_index >/dev/null 2>&1; then
    echo "preview selection parser accepted empty result" >&2
    return 1
  fi

  local query
  query="$(preview_selection_index_query "neo'geo")"
  [[ "$query" == *"neo''geo"* ]] || {
    echo "preview selection query did not quote apostrophe" >&2
    return 1
  }
}
