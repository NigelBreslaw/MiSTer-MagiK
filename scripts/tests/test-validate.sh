#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
export MISTER_VALIDATE_ROUTING_CHILD=1
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
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

base=""
domain="magik-core-tests magik-core-clippy mister-runtime-tests mister-runtime-clippy "
runtime="mister-runtime-tests mister-runtime-clippy "
app="host-tests mister-app-clippy "
assert_plan docs 'docs/catalog.md' "$base"
assert_plan empty '' ''
assert_plan rust 'apps/mister/src/launcher.rs' "format-app-mister ${base}${app}production-ui-check "
assert_plan core 'crates/magik-core/src/platform.rs' "format-magik-core ${base}${domain}${app}production-ui-check "
assert_plan runtime 'mister/platform/runtime/src/fpga.rs' "format-mister-runtime ${base}${runtime}${app}production-ui-check "
assert_plan catalog 'crates/catalog/src/library_db.rs' "format-catalog ${base}${runtime}${app}catalog-tests catalog-clippy production-ui-check mister-tests mister-clippy "
assert_plan slint 'apps/mister/ui/launcher.slint' "${base}production-ui-check "
assert_plan ui-generated 'apps/mister/ui-generated/build.rs' "format-ui-generated ${base}production-ui-check "
assert_plan mister 'mister/tools/host/src/main.rs' "format-mister-host ${base}mister-tests mister-clippy "
assert_plan agent 'mister/tools/agent/src/main.rs' "format-mister-agent ${base}agent-tests agent-clippy "
assert_plan stream 'crates/framebuffer-stream/src/lib.rs' "format-framebuffer-stream ${base}${runtime}${app}production-ui-check framebuffer-stream-tests framebuffer-stream-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check "
assert_plan desktop 'apps/desktop/src/main.rs' "format-desktop ${base}desktop-tests desktop-compiled-check "
assert_plan documentation 'documentation/src/content/docs/index.mdx' "${base}documentation-build "
assert_plan global 'Cargo.lock' "${base}host-tools-fast ${domain}host-tests mister-app-clippy catalog-tests catalog-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy mister-tests mister-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check documentation-build "
assert_plan mixed $'crates/catalog/src/library_db.rs\napps/mister/ui/launcher.slint\nscripts/deploy-rust.sh' "format-catalog ${base}host-tools-fast ${runtime}${app}catalog-tests catalog-clippy production-ui-check mister-tests mister-clippy "
assert_plan deletion 'apps/mister/src/removed.rs' "format-app-mister ${base}${app}production-ui-check "
assert_plan rename $'apps/mister/ui/old-name.slint\napps/mister/ui/new-name.slint' "${base}production-ui-check "
assert_plan build-script 'apps/mister/build-arm.sh' "${base}host-tools-fast ${app}production-ui-check "
assert_plan rust-toolchain 'apps/mister/rust-toolchain.toml' "format ${base}host-tools-fast ${domain}host-tests mister-app-clippy catalog-tests catalog-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy mister-tests mister-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check "
assert_plan workflow '.github/workflows/rust-arm.yml' "${base}host-tools-fast "
assert_plan validator $'scripts/validate\nscripts/tests/test-validate.sh' "${base}host-tools-fast "
printf '%s\n' 'scripts/validate' >"$TMP/validator-live.paths"
actual="$({
  unset MISTER_VALIDATE_ROUTING_CHILD
  "$ROOT/scripts/validate" affected --paths-file "$TMP/validator-live.paths" --print-plan
} | tr '\n' ' ')"
[ "$actual" = "validation-routing host-tools-fast " ] || {
  echo "validator live plan mismatch: $actual" >&2
  exit 1
}
assert_plan format-dedup $'apps/mister/src/launcher.rs\napps/mister/src/input.rs' "format-app-mister ${base}${app}production-ui-check "
assert_plan agent-protocol 'crates/agent-protocol/src/lib.rs' 'format-agent-protocol '
assert_plan media-contract 'crates/media-contract/src/lib.rs' 'format-media-contract '
assert_plan latch-contract 'mister/platform/contracts/latch/src/lib.rs' 'format-latch-contract '
assert_plan scanout-contract 'mister/platform/contracts/scanout/src/lib.rs' 'format-scanout-contract '
assert_plan shared 'crates/framebuffer-stream/src/lib.rs' "format-framebuffer-stream ${base}${runtime}${app}production-ui-check framebuffer-stream-tests framebuffer-stream-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check "

repo="$TMP/rename-repo"
mkdir -p "$repo/apps/mister/src" "$repo/docs"
git -C "$repo" init -q
git -C "$repo" config user.name validator-test
git -C "$repo" config user.email validator-test@example.invalid
printf 'old\n' >"$repo/apps/mister/src/old.rs"
git -C "$repo" add apps/mister/src/old.rs
git -C "$repo" commit -qm initial
git -C "$repo" mv apps/mister/src/old.rs docs/new.md
actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$repo/.git" GIT_WORK_TREE="$repo" \
    "$ROOT/scripts/validate" affected --print-plan
} | tr '\n' ' ')"
expected="format-app-mister ${base}${app}production-ui-check "
if [ "$actual" != "$expected" ]; then
  echo "real staged rename plan mismatch: $actual" >&2
  exit 1
fi

worktree_repo="$TMP/worktree-repo"
mkdir -p \
  "$worktree_repo/apps/mister/src" \
  "$worktree_repo/mister/tools/host/src" \
  "$worktree_repo/mister/tools/agent/src" \
  "$worktree_repo/docs" \
  "$worktree_repo/ignored"
git -C "$worktree_repo" init -q
git -C "$worktree_repo" config user.name validator-test
git -C "$worktree_repo" config user.email validator-test@example.invalid
printf 'ignored/\n' >"$worktree_repo/.gitignore"
printf 'tracked\n' >"$worktree_repo/apps/mister/src/tracked.rs"
printf 'rename\n' >"$worktree_repo/mister/tools/host/src/old.rs"
printf 'delete\n' >"$worktree_repo/mister/tools/agent/src/deleted.rs"
printf 'docs\n' >"$worktree_repo/docs/readme.md"
git -C "$worktree_repo" add .
git -C "$worktree_repo" commit -qm initial

printf 'changed\n' >>"$worktree_repo/apps/mister/src/tracked.rs"
git -C "$worktree_repo" add apps/mister/src/tracked.rs
git -C "$worktree_repo" mv mister/tools/host/src/old.rs mister/tools/host/src/new.rs
rm "$worktree_repo/mister/tools/agent/src/deleted.rs"
printf 'untracked\n' >"$worktree_repo/apps/mister/src/untracked.rs"
printf 'ignored\n' >"$worktree_repo/ignored/not-routed.rs"
actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$worktree_repo/.git" GIT_WORK_TREE="$worktree_repo" \
    "$ROOT/scripts/validate" working-tree --print-plan
} | tr '\n' ' ')"
expected="format-app-mister format-mister-host format-mister-agent ${base}${app}production-ui-check mister-tests mister-clippy agent-tests agent-clippy "
if [ "$actual" != "$expected" ]; then
  echo "working-tree plan mismatch: $actual" >&2
  exit 1
fi

untracked_repo="$TMP/untracked-repo"
mkdir -p \
  "$untracked_repo/docs" \
  "$untracked_repo/mister/tools/agent/src" \
  "$untracked_repo/apps/mister/src"
git -C "$untracked_repo" init -q
git -C "$untracked_repo" config user.name validator-test
git -C "$untracked_repo" config user.email validator-test@example.invalid
printf '/apps/mister/src/ignored.rs\n' >"$untracked_repo/.gitignore"
printf 'docs\n' >"$untracked_repo/docs/readme.md"
git -C "$untracked_repo" add .gitignore docs/readme.md
git -C "$untracked_repo" commit -qm initial
printf 'untracked\n' >"$untracked_repo/mister/tools/agent/src/untracked.rs"
printf 'ignored\n' >"$untracked_repo/apps/mister/src/ignored.rs"
actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$untracked_repo/.git" GIT_WORK_TREE="$untracked_repo" \
    "$ROOT/scripts/validate" working-tree --print-plan
} | tr '\n' ' ')"
expected="format-mister-agent ${base}agent-tests agent-clippy "
if [ "$actual" != "$expected" ]; then
  echo "untracked/ignored plan mismatch: $actual" >&2
  exit 1
fi

assert_command_plan explicit-space \
  "format-app-mister ${base}${app}production-ui-check " \
  "$ROOT/scripts/validate" paths "apps/mister/src/space name.rs" --print-plan

assert_command_plan absolute-file \
  "format-mister-host ${base}mister-tests mister-clippy " \
  "$ROOT/scripts/validate" paths "$ROOT/mister/tools/host/src/main.rs" --print-plan

whole_repo="format ${base}host-tools-fast ${domain}host-tests mister-app-clippy catalog-tests catalog-clippy production-ui-check framebuffer-stream-tests framebuffer-stream-clippy mister-tests mister-clippy agent-tests agent-clippy desktop-tests desktop-compiled-check documentation-build "
assert_command_plan relative-root "$whole_repo" \
  "$ROOT/scripts/validate" paths . --print-plan
assert_command_plan absolute-root "$whole_repo" \
  "$ROOT/scripts/validate" paths "$ROOT" --print-plan
assert_command_plan absolute-root-slash "$whole_repo" \
  "$ROOT/scripts/validate" paths "$ROOT/" --print-plan

actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$worktree_repo/.git" GIT_WORK_TREE="$worktree_repo" \
    "$ROOT/scripts/validate" paths apps/mister mister/tools/host --print-plan
} | tr '\n' ' ')"
expected="format-app-mister format-mister-host ${base}${app}production-ui-check mister-tests mister-clippy "
if [ "$actual" != "$expected" ]; then
  echo "explicit directory plan mismatch: $actual" >&2
  exit 1
fi

actual="$({
  unset GIT_INDEX_FILE GIT_PREFIX
  GIT_DIR="$worktree_repo/.git" GIT_WORK_TREE="$worktree_repo" \
    "$ROOT/scripts/validate" paths "$ROOT/apps/mister" --print-plan
} | tr '\n' ' ')"
expected="format-app-mister ${base}${app}production-ui-check "
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

fake_checks="$TMP/fake-checks"
mkdir -p "$fake_checks"
cat >"$fake_checks/format-app-mister" <<'EOF'
#!/usr/bin/env bash
echo format-marker
EOF
cat >"$fake_checks/host-tools-fast" <<'EOF'
#!/usr/bin/env bash
echo host-marker
EOF
chmod +x "$fake_checks/format-app-mister" "$fake_checks/host-tools-fast"

empty_json="$(MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" "$ROOT/scripts/validate" paths docs/catalog.md --json)"
python3 - "$empty_json" <<'PY'
import json
import sys
value = json.loads(sys.argv[1])
assert value["status"] == "pass"
assert value["checks"] == []
PY

output="$(MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" "$ROOT/scripts/validate" paths apps/mister/src/launcher.rs 2>&1)"
grep -q '^VALIDATION start check=format-app-mister$' <<<"$output"
grep -q '^VALIDATION pass check=format-app-mister duration_ms=' <<<"$output"
grep -q '^PASS format-app-mister' <<<"$output"
if grep -q 'format-marker' <<<"$output"; then
  echo "default validation output leaked full logs" >&2
  exit 1
fi

output="$(MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" "$ROOT/scripts/validate" paths scripts/README.md 2>&1)"
grep -q '^VALIDATION start check=host-tools-fast$' <<<"$output"
grep -q '^VALIDATION pass check=host-tools-fast duration_ms=' <<<"$output"
grep -q '^PASS host-tools-fast' <<<"$output"

verbose_output="$(MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" "$ROOT/scripts/validate" paths apps/mister/src/launcher.rs --verbose)"
grep -q 'format-marker' <<<"$verbose_output"

json_output="$(MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" "$ROOT/scripts/validate" paths scripts/README.md --json)"
python3 - "$json_output" <<'PY'
import json
import sys
value = json.loads(sys.argv[1])
assert value["schema"] == "mister-magik-validation-v1"
assert value["status"] == "pass"
assert [check["name"] for check in value["checks"]] == ["host-tools-fast"]
assert all(check["status"] == "pass" for check in value["checks"])
PY

for group_spec in \
  'quality:format,validation-routing' \
  'domain:magik-core-tests,magik-core-clippy,mister-runtime-tests,mister-runtime-clippy' \
  'app:host-tests,mister-app-clippy,production-ui-check' \
  'catalog:catalog-tests,catalog-clippy' \
  'tools:host-tools,mister-clippy,agent-tests,agent-clippy'; do
  group="${group_spec%%:*}"
  expected_checks="${group_spec#*:}"
  group_json="$(MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" "$ROOT/scripts/validate" full-host --group "$group" --json)"
  python3 - "$group_json" "$expected_checks" <<'PY'
import json
import sys
value = json.loads(sys.argv[1])
assert [check["name"] for check in value["checks"]] == sys.argv[2].split(",")
assert all(check["status"] == "pass" for check in value["checks"])
PY
done

if MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" \
    "$ROOT/scripts/validate" full-host --group unknown >/dev/null 2>&1; then
  echo "full-host accepted an unknown group" >&2
  exit 1
fi

cat >"$fake_checks/host-tools-fast.next" <<'EOF'
#!/usr/bin/env bash
echo failure-marker
exit 7
EOF
chmod +x "$fake_checks/host-tools-fast.next"
mv "$fake_checks/host-tools-fast.next" "$fake_checks/host-tools-fast"
set +e
failure_output="$(
  MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" \
    "$ROOT/scripts/validate" paths scripts/README.md 2>&1
)"
failure_status=$?
set -e
[ "$failure_status" -eq 1 ]
grep -q '^FAIL host-tools-fast' <<<"$failure_output"
grep -q 'failure-marker' <<<"$failure_output"
grep -q '^LOG  ' <<<"$failure_output"

cat >"$fake_checks/host-tools-fast.next" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat >"$fake_checks/host-tests" <<'EOF'
#!/usr/bin/env bash
exit 9
EOF
chmod +x "$fake_checks/host-tools-fast.next" "$fake_checks/host-tests"
mv "$fake_checks/host-tools-fast.next" "$fake_checks/host-tools-fast"
set +e
parallel_json="$(
  MISTER_VALIDATE_FAKE_CHECK_DIR="$fake_checks" \
    "$ROOT/scripts/validate" paths apps/mister/src/launcher.rs mister/tools/agent/src/main.rs --json
)"
parallel_status=$?
set -e
[ "$parallel_status" -eq 1 ]
python3 - "$parallel_json" <<'PY'
import json
import sys
value = json.loads(sys.argv[1])
statuses = {check["name"]: check["status"] for check in value["checks"]}
assert statuses["host-tests"] == "fail"
assert statuses["mister-app-clippy"] == "skip"
assert statuses["agent-tests"] == "pass"
assert value["status"] == "fail"
PY

echo "validate routing tests ok"
