#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
unset GIT_DIR GIT_INDEX_FILE GIT_PREFIX GIT_WORK_TREE
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

assert_command_plan() {
  local name="$1" expected="$2"
  shift 2
  actual="$("$@" | tr '\n' ' ')"
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

worktree_repo="$TMP/worktree-repo"
mkdir -p \
  "$worktree_repo/magik-gui/src" \
  "$worktree_repo/tools/mister/src" \
  "$worktree_repo/tools/magik-agent/src" \
  "$worktree_repo/docs" \
  "$worktree_repo/ignored"
git -C "$worktree_repo" init -q
git -C "$worktree_repo" config user.name validator-test
git -C "$worktree_repo" config user.email validator-test@example.invalid
printf 'ignored/\n' >"$worktree_repo/.gitignore"
printf 'tracked\n' >"$worktree_repo/magik-gui/src/tracked.rs"
printf 'rename\n' >"$worktree_repo/tools/mister/src/old.rs"
printf 'delete\n' >"$worktree_repo/tools/magik-agent/src/deleted.rs"
printf 'docs\n' >"$worktree_repo/docs/readme.md"
git -C "$worktree_repo" add .
git -C "$worktree_repo" commit -qm initial

printf 'changed\n' >>"$worktree_repo/magik-gui/src/tracked.rs"
git -C "$worktree_repo" add magik-gui/src/tracked.rs
git -C "$worktree_repo" mv tools/mister/src/old.rs tools/mister/src/new.rs
rm "$worktree_repo/tools/magik-agent/src/deleted.rs"
printf 'untracked\n' >"$worktree_repo/magik-gui/src/untracked.rs"
printf 'ignored\n' >"$worktree_repo/ignored/not-routed.rs"
actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$worktree_repo/.git" GIT_WORK_TREE="$worktree_repo" \
    "$ROOT/scripts/validate" working-tree --print-plan
} | tr '\n' ' ')"
expected="${base}host-tests magik-gui-clippy production-ui-check mister-tests mister-clippy agent-clippy "
if [ "$actual" != "$expected" ]; then
  echo "working-tree plan mismatch: $actual" >&2
  exit 1
fi

untracked_repo="$TMP/untracked-repo"
mkdir -p \
  "$untracked_repo/docs" \
  "$untracked_repo/tools/magik-agent/src" \
  "$untracked_repo/magik-gui/src"
git -C "$untracked_repo" init -q
git -C "$untracked_repo" config user.name validator-test
git -C "$untracked_repo" config user.email validator-test@example.invalid
printf '/magik-gui/src/ignored.rs\n' >"$untracked_repo/.gitignore"
printf 'docs\n' >"$untracked_repo/docs/readme.md"
git -C "$untracked_repo" add .gitignore docs/readme.md
git -C "$untracked_repo" commit -qm initial
printf 'untracked\n' >"$untracked_repo/tools/magik-agent/src/untracked.rs"
printf 'ignored\n' >"$untracked_repo/magik-gui/src/ignored.rs"
actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$untracked_repo/.git" GIT_WORK_TREE="$untracked_repo" \
    "$ROOT/scripts/validate" working-tree --print-plan
} | tr '\n' ' ')"
expected="${base}agent-clippy "
if [ "$actual" != "$expected" ]; then
  echo "untracked/ignored plan mismatch: $actual" >&2
  exit 1
fi

assert_command_plan explicit-space \
  "${base}host-tests magik-gui-clippy production-ui-check " \
  "$ROOT/scripts/validate" paths "magik-gui/src/space name.rs" --print-plan

assert_command_plan absolute-file \
  "${base}mister-tests mister-clippy " \
  "$ROOT/scripts/validate" paths "$ROOT/tools/mister/src/main.rs" --print-plan

whole_repo="${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check mister-tests mister-clippy agent-clippy "
assert_command_plan relative-root "$whole_repo" \
  "$ROOT/scripts/validate" paths . --print-plan
assert_command_plan absolute-root "$whole_repo" \
  "$ROOT/scripts/validate" paths "$ROOT" --print-plan
assert_command_plan absolute-root-slash "$whole_repo" \
  "$ROOT/scripts/validate" paths "$ROOT/" --print-plan

actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$worktree_repo/.git" GIT_WORK_TREE="$worktree_repo" \
    "$ROOT/scripts/validate" paths magik-gui tools/mister --print-plan
} | tr '\n' ' ')"
expected="${base}host-tests magik-gui-clippy production-ui-check mister-tests mister-clippy "
if [ "$actual" != "$expected" ]; then
  echo "explicit directory plan mismatch: $actual" >&2
  exit 1
fi

actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$worktree_repo/.git" GIT_WORK_TREE="$worktree_repo" \
    "$ROOT/scripts/validate" paths "$ROOT/magik-gui" --print-plan
} | tr '\n' ' ')"
expected="${base}host-tests magik-gui-clippy production-ui-check "
if [ "$actual" != "$expected" ]; then
  echo "absolute directory plan mismatch: $actual" >&2
  exit 1
fi

if "$ROOT/scripts/validate" paths --print-plan >/dev/null 2>&1; then
  echo "paths mode accepted an empty path list" >&2
  exit 1
fi
if "$ROOT/scripts/validate" paths "$TMP/outside.rs" --print-plan >/dev/null 2>&1; then
  echo "paths mode accepted an absolute path outside the repository" >&2
  exit 1
fi
if "$ROOT/scripts/validate" working-tree --paths-file "$TMP/unused" >/dev/null 2>&1; then
  echo "working-tree accepted --paths-file" >&2
  exit 1
fi

echo "validate routing tests ok"
