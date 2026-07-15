#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
unset GIT_INDEX_FILE GIT_PREFIX
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

assert_plan() {
  local name="$1" paths="$2" expected="$3"
  printf '%s\n' "$paths" >"$TMP/$name.paths"
  actual="$("$ROOT/scripts/validate" affected --paths-file "$TMP/$name.paths" --print-plan | tr '\n' ' ')"
  if [ "$actual" != "$expected" ]; then
    echo "$name plan mismatch: $actual" >&2
    exit 1
  fi
}

base="format host-tools-fast "
assert_plan docs 'docs/catalog.md' "$base"
assert_plan rust 'magik-gui/src/launcher.rs' "${base}host-tests magik-gui-clippy production-ui-check "
assert_plan catalog 'magik-gui/catalog/src/library_db.rs' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check "
assert_plan slint 'magik-gui/ui/launcher.slint' "${base}production-ui-check "
assert_plan mister 'tools/mister/src/main.rs' "${base}mister-tests mister-clippy "
assert_plan agent 'tools/magik-agent/src/main.rs' "${base}agent-clippy "
assert_plan global 'Cargo.lock' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check mister-tests mister-clippy agent-clippy "
assert_plan mixed $'magik-gui/catalog/src/library_db.rs\nmagik-gui/ui/launcher.slint\nscripts/deploy-rust.sh' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check "
assert_plan deletion 'magik-gui/src/removed.rs' "${base}host-tests magik-gui-clippy production-ui-check "
assert_plan rename $'magik-gui/ui/old-name.slint\nmagik-gui/ui/new-name.slint' "${base}production-ui-check "
assert_plan build-script 'magik-gui/build-arm.sh' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check "
assert_plan workflow '.github/workflows/rust-arm.yml' "$base"
assert_plan shared 'framebuffer-stream/src/lib.rs' "${base}host-tests magik-gui-clippy production-ui-check agent-clippy "

repo="$TMP/rename-repo"
mkdir -p "$repo/magik-gui/src" "$repo/docs"
git -C "$repo" init -q
git -C "$repo" config user.name validator-test
git -C "$repo" config user.email validator-test@example.invalid
printf 'old\n' >"$repo/magik-gui/src/old.rs"
git -C "$repo" add magik-gui/src/old.rs
git -C "$repo" commit -qm initial
git -C "$repo" mv magik-gui/src/old.rs docs/new.md
actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$repo/.git" GIT_WORK_TREE="$repo" \
    "$ROOT/scripts/validate" affected --print-plan
} | tr '\n' ' ')"
expected="${base}host-tests magik-gui-clippy production-ui-check "
if [ "$actual" != "$expected" ]; then
  echo "real staged rename plan mismatch: $actual" >&2
  exit 1
fi

echo "validate routing tests ok"
