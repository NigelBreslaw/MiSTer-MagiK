#!/usr/bin/env bash
# Public-beta host release gate for MiSTer MagiK.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
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
  --name release-check
  --out-dir "$WORK"
)
if [ -f "$MAIN_BIN" ]; then
  package_args+=(--main-bin "$MAIN_BIN")
  EXPECT_MAIN=1
else
  EXPECT_MAIN=0
fi

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
    "mister-magik/mame.sqlite3",
}
if expect_main:
    required.add("MiSTer_MagiK")

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
