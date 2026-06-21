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

if command -v sqlite3 >/dev/null 2>&1; then
  mkdir -p "$TMP/neogeo/originals"
  sqlite3 "$TMP/neogeo-mame.sqlite3" "
    CREATE TABLE mame_machines (
      setname TEXT PRIMARY KEY,
      parent_setname TEXT,
      title TEXT NOT NULL
    );
    INSERT INTO mame_machines(setname,parent_setname,title) VALUES
      ('kof2002',NULL,'The King of Fighters 2002'),
      ('kf2k2mp','kof2002','The King of Fighters 2002 Magic Plus'),
      ('kf10thep','kof2002','The King of Fighters 10th Anniversary Extra Plus'),
      ('mslug3',NULL,'Metal Slug 3'),
      ('mslug3h','mslug3','Metal Slug 3');
  "
  printf parent >"$TMP/neogeo/originals/kof2002.png"
  printf clone >"$TMP/neogeo/originals/kf2k2mp.png"
  printf clone >"$TMP/neogeo/originals/kf10thep.jpg"
  printf clone >"$TMP/neogeo/originals/mslug3h.png"
  printf orphan >"$TMP/neogeo/originals/orphan.png"
  printf sidecar >"$TMP/neogeo/originals/._orphan.png"
  "$ROOT/scripts/build-neogeo-screenshot-pack.sh" \
    --skip-fetch \
    --stage-only \
    --work-dir "$TMP/neogeo" \
    --mame-sqlite "$TMP/neogeo-mame.sqlite3" >/dev/null
  test -f "$TMP/neogeo/family/kof2002.png"
  test -f "$TMP/neogeo/family/mslug3.png"
  test -f "$TMP/neogeo/family/orphan.png"
  test ! -e "$TMP/neogeo/family/kf2k2mp.png"
  test ! -e "$TMP/neogeo/family/kf10thep.jpg"
  test ! -e "$TMP/neogeo/family/._orphan.png"
fi

if grep -R -E 'scripts/(bench-effects|profile-camera-effects|profile-sprite-effects|profile-text-effects|profile-raster-effects|profile-transition-effects)\.sh|scripts/experiments/(profile-preview-transition-mega|bench-effects|profile-camera-effects|profile-sprite-effects|profile-text-effects|profile-raster-effects|profile-transition-effects)\.sh' \
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
"$ROOT/scripts/bench-toolchain.sh" --self-test
"$ROOT/scripts/profile-preview-scroll.sh" --self-test
env RUSTC_WRAPPER= cargo test --manifest-path "$ROOT/tools/mister/Cargo.toml" --quiet

echo "host tool checks ok"
