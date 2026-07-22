#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURE="$(mktemp -d "${TMPDIR:-/tmp}/agent-launcher.XXXXXX")"
trap 'rm -rf "$FIXTURE"' EXIT

mkdir -p "$FIXTURE/project/src" "$FIXTURE/bin" "$FIXTURE/target/release"
touch "$FIXTURE/project/Cargo.toml" "$FIXTURE/project/Cargo.lock" "$FIXTURE/project/src/main.rs"

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
export MISTER_AGENT_CLI_MANIFEST="$FIXTURE/project/Cargo.toml"
export MISTER_AGENT_CLI_BINARY="$FIXTURE/target/release/agent-cli"
export CARGO_TARGET_DIR="$FIXTURE/target"

output="$($ROOT/scripts/agent plan)"
[[ "$output" == "agent:plan" ]]
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 1 ]]
[[ "$(<"$FIXTURE_CARGO_ARGS")" == "build --release --locked --quiet --manifest-path $MISTER_AGENT_CLI_MANIFEST" ]]
[[ ! -e "$FIXTURE_RUSTC_CALLS" ]]

"$ROOT/scripts/agent" check >/dev/null
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 1 ]]
[[ ! -e "$FIXTURE_RUSTC_CALLS" ]]

sleep 1
touch "$FIXTURE/project/src/main.rs"
"$ROOT/scripts/agent" verify >/dev/null
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 2 ]]

rm -f "$MISTER_AGENT_CLI_BINARY"
export FIXTURE_BUILD_DELAY=0.2
"$ROOT/scripts/agent" one >/dev/null &
first=$!
"$ROOT/scripts/agent" two >/dev/null &
second=$!
wait "$first" "$second"
[[ "$(<"$FIXTURE_BUILD_COUNT")" == 3 ]]

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
