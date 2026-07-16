#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Profile the standalone developer catalog builder directly. Production never
# selects this executable as a backend.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
source "$ROOT/scripts/lib/magik-layout.sh"
source "$ROOT/scripts/lib/mister-supervision-lib.sh"
source "$ROOT/scripts/lib/thread-sampler-lib.sh"
magik_layout_select dev

LABEL=""
SKIP_BUILD=0
case "${1:-}" in
  -h|--help|"")
    echo "usage: scripts/profile-catalog-builder.sh LABEL [--skip-build]"
    exit "$([[ -n "${1:-}" ]] && echo 0 || echo 2)"
    ;;
  *) LABEL="$1"; shift ;;
esac
if [[ "${1:-}" == --skip-build ]]; then
  SKIP_BUILD=1
  shift
fi
if [[ $# -ne 0 ]]; then
  echo "ERROR: unknown argument: $1" >&2
  exit 2
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "ERROR: label must contain only letters, digits, dot, underscore, or dash" >&2
  exit 2
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  "$ROOT/scripts/deploy-catalog-builder.sh"
fi

OUT_DIR="$ROOT/build/catalog-builder-profiles"
OUT="$OUT_DIR/$LABEL.jsonl"
REMOTE_LOG="/tmp/mister-magik-catalog-builder-profile.jsonl"
mkdir -p "$OUT_DIR"
launcher_suspended=0
thread_sample_enabled=1
cleanup() {
  thread_sample_stop
  if [[ "$launcher_suspended" -eq 1 ]]; then
    mister_supervision_command "mister_magik_resume" 0.5 >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

echo "==> Suspending launcher and running standalone builder directly"
mister_suspend_launcher 1 >/dev/null
launcher_suspended=1
thread_sample_start "$LABEL" "standalone-catalog-builder" "$OUT_DIR" 600 "mister-magik-catalog-builder"
"$MISTER" run "
set -e
pids=\$(pidof mister-magik-catalog-builder 2>/dev/null || true)
if test -n \"\$pids\"; then
  kill \$pids 2>/dev/null || true
  attempts=0
  while pidof mister-magik-catalog-builder >/dev/null 2>&1 && test \$attempts -lt 20; do
    sleep 0.1
    attempts=\$((attempts + 1))
  done
  test -z \"\$(pidof mister-magik-catalog-builder 2>/dev/null || true)\"
fi
rm -f '$MISTER_MAGIK_LIBRARY_DB' '$MISTER_MAGIK_APP_DIR/library.summary.json' '$MISTER_MAGIK_APP_DIR/library.nav.lz4b' '$REMOTE_LOG'
'$MISTER_MAGIK_CATALOG_BUILDER' fresh-build >'$REMOTE_LOG' 2>&1
sync
"
thread_sample_stop
thread_sample_collect
"$MISTER" get "$REMOTE_LOG" "$OUT" >/dev/null
mister_supervision_command "mister_magik_resume" 0.5 >/dev/null
launcher_suspended=0

python3 - "$OUT" <<'PY'
import json
import sys

events = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
names = [event.get("event") for event in events]
if not names or names[0] != "handshake" or names[-1] != "done":
    raise SystemExit("standalone builder did not emit a complete event sequence")
failure = next((event for event in events if event.get("event") == "failure"), None)
if failure:
    raise SystemExit(f"standalone builder failed: {failure}")
ready = next(event for event in events if event.get("event") == "timing" and event.get("name") == "builder_catalog_ready")
saved = next(event for event in events if event.get("event") == "timing" and event.get("name") == "builder_persisted")
print(f"catalog_builder_profile ready={ready['detail']} persisted={saved['detail']}")
PY
if [[ ! -s "$thread_sample_local_tsv" ]] || ! awk -F '\t' '$1 == "thread_sample_tsv" && $2 != "sample" && $20 + 0 > 0 { found=1 } END { exit(found ? 0 : 1) }' "$thread_sample_local_tsv"; then
  echo "ERROR: standalone builder profile lacks CPU/RSS/HWM samples" >&2
  exit 1
fi
thread_sample_emit_summary "$LABEL" "standalone-catalog-builder" "$thread_sample_local_tsv"
echo "==> Standalone builder profile: $OUT"
