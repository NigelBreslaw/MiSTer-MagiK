#!/usr/bin/env bash
# Cheap host-side checks for retained shell/Python/AWK tooling.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/MiSTer.ini" <<'EOF'
[MiSTer]
direct_video=1
main=mister-magik-fb

[Menu]
video_mode=4

[MiSTer]
fb_terminal=1
EOF

awk -f "$ROOT/scripts/mister-magik/repair-boot-ini.awk" "$TMP/MiSTer.ini" >"$TMP/repaired.ini"
test "$(grep -c '^\[MiSTer\]' "$TMP/repaired.ini")" -eq 1
grep -q '^direct_video=0$' "$TMP/repaired.ini"
grep -q '^main=MiSTer_MagiK$' "$TMP/repaired.ini"
grep -q '^video_mode=8$' "$TMP/repaired.ini"

awk -f "$ROOT/scripts/mister-magik/restore-stock-ini.awk" "$TMP/repaired.ini" >"$TMP/restored.ini"
if grep -q '^main=MiSTer_MagiK$' "$TMP/restored.ini"; then
  echo "restore-stock-ini.awk left MiSTer_MagiK main= behind" >&2
  exit 1
fi

python3 -m py_compile "$ROOT/scripts/mister_ssh.py" "$ROOT/scripts/raw_to_png.py"
grep -q 'allow_agent=False' "$ROOT/scripts/mister_ssh.py"
grep -q 'look_for_keys=False' "$ROOT/scripts/mister_ssh.py"

for script in \
  "$ROOT/scripts/deploy-rust.sh" \
  "$ROOT/scripts/dev-rust" \
  "$ROOT/magik-gui/build-arm.sh"; do
  bash -n "$script"
done

echo "host tool checks ok"
