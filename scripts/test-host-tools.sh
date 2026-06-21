#!/usr/bin/env bash
# Cheap host-side checks for retained shell/Rust tooling.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/MiSTer.ini" <<'EOF'
[MiSTer]
direct_video=1
main=mister-magik-fb

[arcade_vertical]
direct_video=0
video_mode=14

[Menu]
video_mode=4

[MiSTer]
fb_terminal=1
EOF

cargo run --manifest-path "$ROOT/tools/mister/Cargo.toml" --quiet -- \
  ini-edit-local magik-boot "$TMP/MiSTer.ini" "$TMP/repaired.ini"
grep -q '^direct_video=0$' "$TMP/repaired.ini"
grep -q '^main=MiSTer_MagiK$' "$TMP/repaired.ini"
grep -q '^\[arcade_vertical\]$' "$TMP/repaired.ini"
grep -q '^video_mode=14$' "$TMP/repaired.ini"
grep -q '^direct_video=0$' "$TMP/repaired.ini"
grep -q '^video_mode=8$' "$TMP/repaired.ini"

for script in \
  "$ROOT/scripts/check-no-main-kill.sh" \
  "$ROOT/scripts/check-no-direct-arcade-scene.sh" \
  "$ROOT/scripts/bench-toolchain.sh" \
  "$ROOT/scripts/build-mister-agent.sh" \
  "$ROOT/scripts/build-neogeo-screenshot-pack.sh" \
  "$ROOT/scripts/deploy-rust.sh" \
  "$ROOT/scripts/device-catalog-acceptance.sh" \
  "$ROOT/scripts/device-release-acceptance.sh" \
  "$ROOT/scripts/dev-rust" \
  "$ROOT/scripts/install-slint-boot.sh" \
  "$ROOT/scripts/mister" \
  "$ROOT/scripts/mister-asset-diagnostics.sh" \
  "$ROOT/scripts/mister-magik-agent.sh" \
  "$ROOT/scripts/profile-preview-scroll.sh" \
  "$ROOT/scripts/restore-stock-boot.sh" \
  "$ROOT/magik-gui/build-arm.sh"; do
  bash -n "$script"
done

for script in "$ROOT"/scripts/experiments/*.sh; do
  bash -n "$script"
done

if grep -R -E 'scripts/(bench-effects|profile-preview-transition-mega|profile-camera-effects|profile-sprite-effects|profile-text-effects|profile-raster-effects|profile-transition-effects)\.sh' \
  "$ROOT/AGENTS.md" "$ROOT/docs/benchmarking.md" "$ROOT/magik-gui/BUILD.md" "$ROOT/magik-gui/ui/bench/README.md" >/dev/null; then
  echo "old effect experiment script path found in current benchmark docs" >&2
  exit 1
fi

"$ROOT/scripts/device-release-acceptance.sh" --help >/dev/null
if "$ROOT/scripts/device-release-acceptance.sh" --tiers nope >/dev/null 2>&1; then
  echo "expected invalid tier to fail" >&2
  exit 1
fi
if "$ROOT/scripts/device-release-acceptance.sh" --fast --tiers health >/dev/null 2>&1; then
  echo "expected --fast plus --tiers to fail" >&2
  exit 1
fi

"$ROOT/scripts/check-no-main-kill.sh"
"$ROOT/scripts/check-no-direct-arcade-scene.sh"
"$ROOT/scripts/profile-preview-scroll.sh" --self-test
env RUSTC_WRAPPER= cargo test --manifest-path "$ROOT/tools/mister/Cargo.toml" --quiet
python3 "$ROOT/scripts/import-wikipedia-neogeo-screenshots.py" --self-test

echo "host tool checks ok"
