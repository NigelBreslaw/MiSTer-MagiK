#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURE="$(mktemp -d "${TMPDIR:-/tmp}/shared-cargo-cache.XXXXXX")"
PRIMARY="$FIXTURE/primary"
LINKED="$FIXTURE/linked"
BIN="$FIXTURE/bin"
trap 'rm -rf "$FIXTURE"' EXIT

mkdir -p \
  "$PRIMARY/scripts/lib" \
  "$PRIMARY/sample-tool/src" \
  "$PRIMARY/apps/mister/src" \
  "$PRIMARY/crates/catalog/src" \
  "$PRIMARY/crates/catalog/data" \
  "$PRIMARY/crates/media-contract/src" \
  "$PRIMARY/crates/sample-wire/src" \
  "$BIN"
cp "$ROOT/scripts/cargo" "$PRIMARY/scripts/cargo"
cp "$ROOT/scripts/lib/shared-worktree-cache.sh" \
  "$PRIMARY/scripts/lib/shared-worktree-cache.sh"
chmod +x "$PRIMARY/scripts/cargo"
touch \
  "$PRIMARY/sample-tool/Cargo.toml" \
  "$PRIMARY/sample-tool/Cargo.lock" \
  "$PRIMARY/sample-tool/src/main.rs" \
  "$PRIMARY/apps/mister/Cargo.toml" \
  "$PRIMARY/apps/mister/src/main.rs" \
  "$PRIMARY/crates/catalog/Cargo.toml" \
  "$PRIMARY/crates/catalog/src/lib.rs" \
  "$PRIMARY/crates/catalog/data/system.json" \
  "$PRIMARY/crates/media-contract/Cargo.toml" \
  "$PRIMARY/crates/media-contract/src/lib.rs" \
  "$PRIMARY/crates/sample-wire/Cargo.toml" \
  "$PRIMARY/crates/sample-wire/src/lib.rs"

git init -q "$PRIMARY"
git -C "$PRIMARY" config user.email test@example.invalid
git -C "$PRIMARY" config user.name Test
git -C "$PRIMARY" add .
git -C "$PRIMARY" commit -qm fixture
git -C "$PRIMARY" branch -M main
git -C "$PRIMARY" worktree add -q -b feature "$LINKED" main
PRIMARY_PHYSICAL="$(cd "$PRIMARY" && pwd -P)"

cat >"$BIN/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == locate-project ]]; then
  manifest_path="Cargo.toml"
  arguments=("$@")
  for ((index = 0; index < ${#arguments[@]}; index += 1)); do
    case "${arguments[$index]}" in
      --manifest-path) manifest_path="${arguments[$((index + 1))]}" ;;
      --manifest-path=*) manifest_path="${arguments[$index]#--manifest-path=}" ;;
    esac
  done
  printf '%s/%s\n' "$PWD" "$manifest_path"
  exit 0
fi
printf '%s\n' "${CARGO_TARGET_DIR:-unset}" >"$FIXTURE_CARGO_TARGET"
printf '%s\n' "$*" >"$FIXTURE_CARGO_ARGS"
count=0
[[ ! -f "$FIXTURE_BUILD_COUNT" ]] || count="$(<"$FIXTURE_BUILD_COUNT")"
printf '%s\n' "$((count + 1))" >"$FIXTURE_BUILD_COUNT"
if [[ "${1:-}" == build ]]; then
  mkdir -p "$CARGO_TARGET_DIR/debug"
  cat >"$CARGO_TARGET_DIR/debug/sample-tool" <<'BIN'
#!/usr/bin/env bash
printf 'agent:%s\n' "$*"
BIN
  chmod +x "$CARGO_TARGET_DIR/debug/sample-tool"
fi
EOF
chmod +x "$BIN/cargo"

export PATH="$BIN:$PATH"
export FIXTURE_BUILD_COUNT="$FIXTURE/build-count"
export FIXTURE_CARGO_ARGS="$FIXTURE/cargo-args"
export FIXTURE_CARGO_TARGET="$FIXTURE/cargo-target"

(
  cd "$LINKED"
  scripts/cargo test --manifest-path sample-tool/Cargo.toml
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == "$PRIMARY_PHYSICAL/sample-tool/target" ]]

(
  cd "$LINKED"
  scripts/cargo check --manifest-path apps/mister/Cargo.toml
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == "$PRIMARY_PHYSICAL/apps/mister/target" ]]

(
  cd "$LINKED"
  CARGO_TARGET_DIR="$FIXTURE/explicit" \
    scripts/cargo check --manifest-path sample-tool/Cargo.toml
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == "$FIXTURE/explicit" ]]

(
  cd "$LINKED"
  scripts/cargo check --manifest-path sample-tool/Cargo.toml \
    --target-dir "$FIXTURE/argument"
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == unset ]]
