#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURE="$(mktemp -d "${TMPDIR:-/tmp}/agent-launcher.XXXXXX")"
trap 'rm -rf "$FIXTURE"' EXIT

mkdir -p \
  "$FIXTURE/agent-cli/src" \
  "$FIXTURE/crates/catalog/src" \
  "$FIXTURE/crates/catalog/data" \
  "$FIXTURE/crates/media-contract/src" \
  "$FIXTURE/crates/agent-protocol/src" \
  "$FIXTURE/bin" \
  "$FIXTURE/target/debug" \
  "$FIXTURE/target/release"
touch \
  "$FIXTURE/agent-cli/Cargo.toml" \
  "$FIXTURE/agent-cli/Cargo.lock" \
  "$FIXTURE/agent-cli/src/main.rs" \
  "$FIXTURE/crates/catalog/Cargo.toml" \
  "$FIXTURE/crates/catalog/src/lib.rs" \
  "$FIXTURE/crates/catalog/data/system.json" \
  "$FIXTURE/crates/media-contract/Cargo.toml" \
  "$FIXTURE/crates/media-contract/src/lib.rs" \
  "$FIXTURE/crates/agent-protocol/Cargo.toml" \
  "$FIXTURE/crates/agent-protocol/src/lib.rs"

cat >"$FIXTURE/bin/rustc" <<'EOF'
#!/usr/bin/env bash
printf 'invoked\n' >>"$FIXTURE_RUSTC_CALLS"
exit 99
EOF
cat >"$FIXTURE/bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
count_file="$FIXTURE_BUILD_COUNT"
printf '%s\n' "$*" >"$FIXTURE_CARGO_ARGS"
count=0
[[ ! -f "$count_file" ]] || count="$(<"$count_file")"
printf '%s\n' "$((count + 1))" >"$count_file"
sleep "${FIXTURE_BUILD_DELAY:-0}"
cat >"$MISTER_AGENT_CLI_BINARY" <<'BIN'
#!/usr/bin/env bash
printf 'agent:%s\n' "$*"
BIN
chmod +x "$MISTER_AGENT_CLI_BINARY"
EOF
chmod +x "$FIXTURE/bin/rustc" "$FIXTURE/bin/cargo"

export PATH="$FIXTURE/bin:$PATH"
export FIXTURE_BUILD_COUNT="$FIXTURE/build-count"
export FIXTURE_CARGO_ARGS="$FIXTURE/cargo-args"
export FIXTURE_RUSTC_CALLS="$FIXTURE/rustc-calls"
export MISTER_AGENT_CLI_MANIFEST="$FIXTURE/agent-cli/Cargo.toml"
export MISTER_AGENT_CLI_BINARY="$FIXTURE/target/debug/agent-cli"
export CARGO_TARGET_DIR="$FIXTURE/target"

output="$($ROOT/scripts/agent bootstrap)"
[[ "$output" == "agent:bootstrap" ]]
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 1 ]]
[[ "$(<"$FIXTURE_CARGO_ARGS")" == "build --locked --quiet --manifest-path $MISTER_AGENT_CLI_MANIFEST" ]]
[[ ! -e "$FIXTURE_RUSTC_CALLS" ]]

"$ROOT/scripts/agent" check >/dev/null
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 1 ]]
[[ ! -e "$FIXTURE_RUSTC_CALLS" ]]

export MISTER_AGENT_CLI_PROFILE=debug
unset MISTER_AGENT_CLI_BINARY
output="$($ROOT/scripts/agent verify)"
[[ "$output" == "agent:verify" ]]
[[ -x "$FIXTURE/target/debug/agent-cli" ]]
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 1 ]]
[[ "$(<"$FIXTURE_CARGO_ARGS")" == "build --locked --quiet --manifest-path $MISTER_AGENT_CLI_MANIFEST" ]]

export MISTER_AGENT_CLI_PROFILE=release
export MISTER_AGENT_CLI_BINARY="$FIXTURE/target/release/agent-cli"

output="$($ROOT/scripts/agent release)"
[[ "$output" == "agent:release" ]]
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 2 ]]
[[ "$(<"$FIXTURE_CARGO_ARGS")" == "build --release --locked --quiet --manifest-path $MISTER_AGENT_CLI_MANIFEST" ]]

sleep 1
touch "$FIXTURE/agent-cli/src/main.rs"
"$ROOT/scripts/agent" verify >/dev/null
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 3 ]]

sleep 1
touch "$FIXTURE/agent-cli/Cargo.toml"
"$ROOT/scripts/agent" manifest >/dev/null
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 4 ]]

sleep 1
touch "$FIXTURE/agent-cli/Cargo.lock"
"$ROOT/scripts/agent" check >/dev/null
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 5 ]]

sleep 1
touch "$FIXTURE/crates/catalog/src/lib.rs"
"$ROOT/scripts/agent" verify >/dev/null
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 6 ]]

rm -f "$MISTER_AGENT_CLI_BINARY"
export FIXTURE_BUILD_DELAY=0.2
"$ROOT/scripts/agent" one >/dev/null &
first=$!
"$ROOT/scripts/agent" two >/dev/null &
second=$!
wait "$first" "$second"
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 7 ]]

rm -f "$MISTER_AGENT_CLI_BINARY"
cat >"$FIXTURE/bin/cargo" <<'EOF'
#!/usr/bin/env bash
exit 23
EOF
chmod +x "$FIXTURE/bin/cargo"
if "$ROOT/scripts/agent" failure >/dev/null 2>&1; then
  echo "launcher accepted a failed build" >&2
  exit 1
fi
