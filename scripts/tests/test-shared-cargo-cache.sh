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
  "$PRIMARY/agent-cli/src" \
  "$PRIMARY/apps/mister/src" \
  "$PRIMARY/crates/catalog/src" \
  "$PRIMARY/crates/catalog/data" \
  "$PRIMARY/crates/media-contract/src" \
  "$PRIMARY/crates/agent-protocol/src" \
  "$BIN"
cp "$ROOT/scripts/agent" "$PRIMARY/scripts/agent"
cp "$ROOT/scripts/cargo" "$PRIMARY/scripts/cargo"
cp "$ROOT/scripts/lib/shared-worktree-cache.sh" \
  "$PRIMARY/scripts/lib/shared-worktree-cache.sh"
chmod +x "$PRIMARY/scripts/agent" "$PRIMARY/scripts/cargo"
touch \
  "$PRIMARY/agent-cli/Cargo.toml" \
  "$PRIMARY/agent-cli/Cargo.lock" \
  "$PRIMARY/agent-cli/src/main.rs" \
  "$PRIMARY/apps/mister/Cargo.toml" \
  "$PRIMARY/apps/mister/src/main.rs" \
  "$PRIMARY/crates/catalog/Cargo.toml" \
  "$PRIMARY/crates/catalog/src/lib.rs" \
  "$PRIMARY/crates/catalog/data/system.json" \
  "$PRIMARY/crates/media-contract/Cargo.toml" \
  "$PRIMARY/crates/media-contract/src/lib.rs" \
  "$PRIMARY/crates/agent-protocol/Cargo.toml" \
  "$PRIMARY/crates/agent-protocol/src/lib.rs"

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
  cat >"$CARGO_TARGET_DIR/debug/agent-cli" <<'BIN'
#!/usr/bin/env bash
printf 'agent:%s\n' "$*"
BIN
  chmod +x "$CARGO_TARGET_DIR/debug/agent-cli"
fi
EOF
chmod +x "$BIN/cargo"

export PATH="$BIN:$PATH"
export FIXTURE_BUILD_COUNT="$FIXTURE/build-count"
export FIXTURE_CARGO_ARGS="$FIXTURE/cargo-args"
export FIXTURE_CARGO_TARGET="$FIXTURE/cargo-target"

(
  cd "$LINKED"
  scripts/cargo test --manifest-path agent-cli/Cargo.toml
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == "$PRIMARY_PHYSICAL/agent-cli/target" ]]

(
  cd "$LINKED"
  scripts/cargo check --manifest-path apps/mister/Cargo.toml
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == "$PRIMARY_PHYSICAL/apps/mister/target" ]]

(
  cd "$LINKED"
  CARGO_TARGET_DIR="$FIXTURE/explicit" \
    scripts/cargo check --manifest-path agent-cli/Cargo.toml
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == "$FIXTURE/explicit" ]]

(
  cd "$LINKED"
  scripts/cargo check --manifest-path agent-cli/Cargo.toml \
    --target-dir "$FIXTURE/argument"
)
[[ "$(<"$FIXTURE_CARGO_TARGET")" == unset ]]

rm -f "$FIXTURE_BUILD_COUNT"
output="$(cd "$LINKED" && scripts/agent one)"
[[ "$output" == "agent:one" ]]
[[ "$(<"$FIXTURE_CARGO_TARGET")" == "$PRIMARY_PHYSICAL/agent-cli/target" ]]
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 1 ]]

output="$(cd "$PRIMARY" && scripts/agent two)"
[[ "$output" == "agent:two" ]]
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 1 ]]

printf 'changed\n' >"$LINKED/agent-cli/src/main.rs"
output="$(cd "$LINKED" && scripts/agent three)"
[[ "$output" == "agent:three" ]]
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 2 ]]
