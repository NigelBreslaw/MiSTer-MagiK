#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/checks"

cat >"$TMP/checks/host-tools-fast" <<'EOF'
#!/usr/bin/env bash
i=1
while [ "$i" -le 30 ]; do
  printf 'failure-line-%02d\n' "$i"
  i=$((i + 1))
done
exit 7
EOF
chmod +x "$TMP/checks/host-tools-fast"

run_validation() {
  MISTER_VALIDATE_ROUTING_CHILD=1 MISTER_VALIDATE_FAKE_CHECK_DIR="$TMP/checks" \
    "$ROOT/scripts/validate" paths scripts/README.md "$@"
}

set +e
compact="$(run_validation 2>&1)"
compact_status=$?
verbose="$(run_validation --verbose 2>&1)"
verbose_status=$?
json="$(run_validation --json 2>/dev/null)"
json_status=$?
set -e

[[ "$compact_status" -eq 1 && "$verbose_status" -eq 1 && "$json_status" -eq 1 ]]
grep -q 'failure-line-11' <<<"$compact"
grep -q 'failure-line-30' <<<"$compact"
if grep -q 'failure-line-10' <<<"$compact"; then
  echo "default validation failure output exceeded 20 log lines" >&2
  exit 1
fi
grep -q '^LOG  ' <<<"$compact"
grep -q 'failure-line-01' <<<"$verbose"
python3 - "$json" <<'PY'
import json, sys
value = json.loads(sys.argv[1])
assert value["status"] == "fail"
assert len(value["checks"]) == 1
assert value["checks"][0]["exit_code"] == 7
assert value["checks"][0]["log_path"].endswith("host-tools-fast.log")
PY

echo "validation output self-test ok"
