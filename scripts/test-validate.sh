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
assert_plan catalog 'magik-gui/catalog/src/library_db.rs' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check mister-tests mister-clippy "
assert_plan slint 'magik-gui/ui/launcher.slint' "${base}production-ui-check "
assert_plan ui-generated 'magik-gui/ui-generated/build.rs' "${base}production-ui-check "
assert_plan mister 'tools/mister/src/main.rs' "${base}mister-tests mister-clippy "
assert_plan agent 'tools/magik-agent/src/main.rs' "${base}agent-tests agent-clippy "
assert_plan stream 'framebuffer-stream/src/lib.rs' "${base}host-tests magik-gui-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check "
assert_plan desktop 'desktop/src/main.rs' "${base}desktop-tests desktop-compiled-check "
assert_plan documentation 'documentation/src/content/docs/index.mdx' "${base}documentation-build "
assert_plan global 'Cargo.lock' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy mister-tests mister-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check documentation-build "
assert_plan mixed $'magik-gui/catalog/src/library_db.rs\nmagik-gui/ui/launcher.slint\nscripts/deploy-rust.sh' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check mister-tests mister-clippy "
assert_plan deletion 'magik-gui/src/removed.rs' "${base}host-tests magik-gui-clippy production-ui-check "
assert_plan rename $'magik-gui/ui/old-name.slint\nmagik-gui/ui/new-name.slint' "${base}production-ui-check "
assert_plan build-script 'magik-gui/build-arm.sh' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check "
assert_plan rust-toolchain 'magik-gui/rust-toolchain.toml' "${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy mister-tests mister-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check "
assert_plan workflow '.github/workflows/rust-arm.yml' "$base"
assert_plan shared 'framebuffer-stream/src/lib.rs' "${base}host-tests magik-gui-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check "

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
expected="${base}host-tests magik-gui-clippy production-ui-check mister-tests mister-clippy agent-tests agent-clippy "
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
expected="${base}agent-tests agent-clippy "
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

whole_repo="${base}host-tests magik-gui-clippy catalog-tests catalog-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy mister-tests mister-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check documentation-build "
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

prereq_bin="$TMP/prereq-bin"
prereq_modules="$TMP/prereq-node-modules"
mkdir -p "$prereq_bin" "$prereq_modules/.pnpm" "$prereq_modules/.bin"
cat >"$prereq_modules/.bin/astro" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$prereq_modules/.bin/astro"
cat >"$prereq_bin/node" <<'EOF'
#!/usr/bin/env bash
printf 'v24.0.0\n'
EOF
cat >"$prereq_bin/corepack" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = pnpm ] && [ "$2" = --version ]; then
  printf '11.10.0\n'
  exit 0
fi
exit 2
EOF
chmod +x "$prereq_bin/node" "$prereq_bin/corepack"
PATH="$prereq_bin:/usr/bin:/bin" \
  MISTER_DOCUMENTATION_NODE_MODULES="$prereq_modules" \
  MISTER_VALIDATE_SELF_TEST=documentation-prerequisites \
  "$ROOT/scripts/validate"

cat >"$prereq_bin/node" <<'EOF'
#!/usr/bin/env bash
printf 'v21.9.0\n'
EOF
chmod +x "$prereq_bin/node"
if PATH="$prereq_bin:/usr/bin:/bin" \
    MISTER_DOCUMENTATION_NODE_MODULES="$prereq_modules" \
    MISTER_VALIDATE_SELF_TEST=documentation-prerequisites \
    "$ROOT/scripts/validate" >/dev/null 2>&1; then
  echo "documentation prerequisites accepted Node.js older than 22" >&2
  exit 1
fi

cat >"$prereq_bin/node" <<'EOF'
#!/usr/bin/env bash
printf 'v24.0.0\n'
EOF
cat >"$prereq_bin/corepack" <<'EOF'
#!/usr/bin/env bash
printf '11.9.0\n'
EOF
chmod +x "$prereq_bin/node" "$prereq_bin/corepack"
if PATH="$prereq_bin:/usr/bin:/bin" \
    MISTER_DOCUMENTATION_NODE_MODULES="$prereq_modules" \
    MISTER_VALIDATE_SELF_TEST=documentation-prerequisites \
    "$ROOT/scripts/validate" >/dev/null 2>&1; then
  echo "documentation prerequisites accepted the wrong pnpm version" >&2
  exit 1
fi

cat >"$prereq_bin/corepack" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = pnpm ] && [ "$2" = --version ]; then
  printf '11.10.0\n'
  exit 0
fi
exit 2
EOF
chmod +x "$prereq_bin/corepack"
if PATH="$prereq_bin:/usr/bin:/bin" \
    MISTER_DOCUMENTATION_NODE_MODULES="$TMP/empty-node-modules" \
    MISTER_VALIDATE_SELF_TEST=documentation-prerequisites \
    "$ROOT/scripts/validate" >/dev/null 2>&1; then
  echo "documentation prerequisites accepted an empty dependency directory" >&2
  exit 1
fi

mkdir -p "$TMP/partial-node-modules/.pnpm"
if PATH="$prereq_bin:/usr/bin:/bin" \
    MISTER_DOCUMENTATION_NODE_MODULES="$TMP/partial-node-modules" \
    MISTER_VALIDATE_SELF_TEST=documentation-prerequisites \
    "$ROOT/scripts/validate" >/dev/null 2>&1; then
  echo "documentation prerequisites accepted partial dependencies" >&2
  exit 1
fi

echo "validate routing tests ok"
