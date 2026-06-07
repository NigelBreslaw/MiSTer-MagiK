#!/usr/bin/env bash
# Cheap host-side checks for retained shell/AWK/Rust tooling.
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

for script in \
  "$ROOT/scripts/bench-toolchain.sh" \
  "$ROOT/scripts/deploy-rust.sh" \
  "$ROOT/scripts/dev-rust" \
  "$ROOT/scripts/install-slint-boot.sh" \
  "$ROOT/scripts/mister" \
  "$ROOT/scripts/restore-stock-boot.sh" \
  "$ROOT/magik-gui/build-arm.sh"; do
  bash -n "$script"
done

env RUSTC_WRAPPER= cargo test --manifest-path "$ROOT/tools/mister/Cargo.toml" --quiet

echo "host tool checks ok"
