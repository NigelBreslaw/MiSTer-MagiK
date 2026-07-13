#!/usr/bin/env bash
# Public-beta host release gate for MiSTer MagiK.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
CATALOG_BUILDER="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-catalog-builder"
WORK="$ROOT/build/release-check-host"
MAIN_BIN="${MISTER_MAIN_BIN:-$ROOT/../Main_MiSTer/bin/MiSTer}"

usage() {
  cat <<'EOF'
usage: scripts/release-check-host.sh

Runs the host/build side of the public-beta release gate:
  - Rust formatting
  - host logic tests
  - catalog crate tests
  - host tool tests
  - clippy for app host logic, catalog, tools, and agent
  - ARM release-device build
  - ARM shared-library check
  - distribution package dry-run and zip layout validation

Set MISTER_MAIN_BIN=/path/to/MiSTer to include a Main_MiSTer fork binary in the
package dry-run. If the default sibling checkout binary exists, it is included.
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi
if [ "$#" -gt 0 ]; then
  echo "ERROR: unknown argument: $1" >&2
  usage >&2
  exit 2
fi

step() {
  echo
  echo "==> $*"
}

step "Rust format"
"$ROOT/scripts/dev-rust" fmt

step "Host logic tests"
"$ROOT/scripts/dev-rust" test

step "Catalog crate tests"
cargo test --manifest-path "$ROOT/magik-gui/catalog/Cargo.toml"

step "Host tool tests"
"$ROOT/scripts/dev-rust" host-tools

step "Clippy magik-gui host logic"
(
  cd "$ROOT/magik-gui"
  cargo clippy --lib --no-default-features -- -D warnings
)

step "Clippy catalog crate"
cargo clippy --manifest-path "$ROOT/magik-gui/catalog/Cargo.toml" --all-targets -- -D warnings

step "Clippy host tools"
cargo clippy --manifest-path "$ROOT/tools/mister/Cargo.toml" --all-targets -- -D warnings

step "Clippy MagiK agent"
cargo clippy --manifest-path "$ROOT/tools/magik-agent/Cargo.toml" --all-targets -- -D warnings

step "ARM release-device build"
"$ROOT/magik-gui/build-arm.sh" --device

step "ARM shared-library check"
"$ROOT/magik-gui/scripts/check-arm-shared-libs.sh" "$BIN"

step "Distribution package dry-run"
rm -rf "$WORK"
mkdir -p "$WORK"
MAME_SQLITE="$WORK/mame.sqlite3"
if command -v sqlite3 >/dev/null 2>&1; then
  sqlite3 "$MAME_SQLITE" \
    "CREATE TABLE release_check(name TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO release_check VALUES('kind','public-beta-host-gate');"
else
  printf 'mister-magik release-check metadata\n' > "$MAME_SQLITE"
fi

package_args=(
  --binary "$BIN"
  --mame-sqlite "$MAME_SQLITE"
  --mame-source-ref release-check-fixture
  --name release-check
  --out-dir "$WORK"
)
if [ -f "$MAIN_BIN" ]; then
  MAIN_SOURCE_REVISION="$(git -C "$(dirname "$MAIN_BIN")/.." rev-parse HEAD)"
else
  MAIN_BIN="$WORK/MiSTer_MagiK"
  cp "$BIN" "$MAIN_BIN"
  MAIN_SOURCE_REVISION="1111111111111111111111111111111111111111"
fi
MAGIK_REVISION="2222222222222222222222222222222222222222"
MENU_REVISION="3333333333333333333333333333333333333333"
printf 'module release-check\n' > "$WORK/mister_magik_scanout_slots.ko"
printf 'rbf release-check\n' > "$WORK/menu-magik-vblank-latch.rbf"
CONTRACT="$(printf release-check-contract | sha256sum | awk '{print $1}')"
printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer fixture\n' \
  "$CONTRACT" "$(sha256sum "$WORK/mister_magik_scanout_slots.ko" | awk '{print $1}')" > "$WORK/scanout.metadata.txt"
printf 'format=mister-magik-fpga-release-v1\nplatform_contract_sha256=%s\nmagik_commit=%s\nsource_commit=%s\nrbf_sha256=%s\n' \
  "$CONTRACT" "$MAGIK_REVISION" "$MENU_REVISION" \
  "$(sha256sum "$WORK/menu-magik-vblank-latch.rbf" | awk '{print $1}')" > "$WORK/latch.metadata.txt"
"$ROOT/scripts/platform-manifest.py" generate \
  --output "$WORK/platform-v1.manifest" --main "$MAIN_BIN" --gui "$BIN" \
  --catalog-builder "$CATALOG_BUILDER" \
  --scanout-module "$WORK/mister_magik_scanout_slots.ko" --scanout-metadata "$WORK/scanout.metadata.txt" \
  --latch-rbf "$WORK/menu-magik-vblank-latch.rbf" --latch-metadata "$WORK/latch.metadata.txt" \
  --main-revision "$MAIN_SOURCE_REVISION" --magik-revision "$MAGIK_REVISION" >/dev/null
package_args+=(
  --catalog-builder "$CATALOG_BUILDER"
  --main-bin "$MAIN_BIN" --main-source-revision "$MAIN_SOURCE_REVISION"
  --scanout-module "$WORK/mister_magik_scanout_slots.ko"
  --scanout-metadata "$WORK/scanout.metadata.txt"
  --latch-rbf "$WORK/menu-magik-vblank-latch.rbf"
  --latch-metadata "$WORK/latch.metadata.txt"
  --platform-manifest "$WORK/platform-v1.manifest"
)
EXPECT_MAIN=1

ZIP="$("$ROOT/scripts/package-distribution.sh" "${package_args[@]}")"
export ZIP EXPECT_MAIN
python3 - <<'PY'
import os
import sys
import zipfile

zip_path = os.environ["ZIP"]
expect_main = os.environ["EXPECT_MAIN"] == "1"
required = {
    "Scripts/mister-magik.sh",
    "mister-magik/mister-magik-fb",
    "mister-magik/mister-magik-catalog-builder",
    "mister-magik/mame.sqlite3",
    "THIRD-PARTY-NOTICES.txt",
    "SOURCE-OFFER.txt",
    "licenses/MiSTer-MagiK-GPL-3.0-or-later.txt",
    "licenses/RUST-LIBRARIES.txt",
    "licenses/FFMPEG-LGPL-2.1-or-later.txt",
    "licenses/PRESS-START-2P-OFL-1.1.txt",
}
if expect_main:
    required.add("MiSTer_MagiK")
    required.update({
        "mister-magik/platform-v1.manifest",
        "mister-magik/mister_magik_scanout_slots.ko",
        "mister-magik/mister_magik_scanout_slots.metadata.txt",
        "mister-magik/fpga/menu-magik-vblank-latch.rbf",
        "mister-magik/fpga/menu-magik-vblank-latch.metadata.txt",
    })

with zipfile.ZipFile(zip_path) as zf:
    names = set(zf.namelist())
missing = sorted(required - names)
if missing:
    print(f"package validation failed: missing {', '.join(missing)}", file=sys.stderr)
    sys.exit(1)

print(f"package validation ok: {zip_path}")
PY

echo
echo "host release gate: ok"
