#!/usr/bin/env bash
# Canned library identity/asset diagnostics for a deployed MiSTer MagiK DB.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
SETNAME="${1:-1941}"

if [[ ! "$SETNAME" =~ ^[A-Za-z0-9_]+$ ]]; then
  echo "setname must be alphanumeric/underscore only" >&2
  exit 2
fi

echo "== family: $SETNAME =="
"$MISTER" db "
WITH target AS (
  SELECT COALESCE(family_id, identity_id) AS family_id
  FROM launchable_identities
  WHERE namespace='mame' AND identity_id='$SETNAME'
  LIMIT 1
)
SELECT l.system_id,
       i.identity_id,
       i.family_id,
       COALESCE(i.metadata_title, l.title) AS title
FROM launchable_identities i
JOIN launchables l ON l.launchable_id=i.launchable_id
JOIN target t ON t.family_id=COALESCE(i.family_id, i.identity_id)
WHERE i.namespace='mame'
ORDER BY i.identity_id;
"

echo "== missing preferred screenshots by system =="
"$MISTER" db "
SELECT system_id, count(*) AS missing
FROM ui_arcade_preferred
WHERE has_image=0
GROUP BY system_id
ORDER BY system_id;
"

echo "== asset link reasons =="
"$MISTER" db "
SELECT system_id, asset_link_reason, count(*) AS rows
FROM ui_arcade_variants
GROUP BY system_id, asset_link_reason
ORDER BY system_id, asset_link_reason;
"

echo "== smoke: 1941 / 1942 / mslug3 =="
"$MISTER" db "
SELECT system_id,
       title,
       identity_id,
       family_id,
       asset_key,
       asset_link_reason,
       preferred_reason
FROM ui_arcade_preferred
WHERE identity_id IN ('1941','1942','mslug3')
   OR family_id IN ('1941','1942','mslug3')
ORDER BY system_id, family_id, identity_id;
"
