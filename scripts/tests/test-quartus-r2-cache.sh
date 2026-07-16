#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/bin" "$TMP/remote" "$TMP/cache/docker-intelFPGA_lite/17.0/quartus/bin"
printf '#!/bin/sh\nexit 0\n' >"$TMP/cache/docker-intelFPGA_lite/17.0/quartus/bin/quartus_sh"
chmod +x "$TMP/cache/docker-intelFPGA_lite/17.0/quartus/bin/quartus_sh"
printf 'Quartus Prime Version 17.0.0 Build 595\n' >"$TMP/cache/docker-intelFPGA_lite/version.txt"
cp "$ROOT/scripts/install-quartus-lite-docker.sh" "$TMP/installer.sh"

cat >"$TMP/bin/aws" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${FAKE_AWS_DENY:-0}" == 1 ]]; then echo 'AccessDenied' >&2; exit 1; fi
while [[ $# -gt 0 && "$1" != s3 && "$1" != s3api ]]; do shift; done
command_name="$1"; shift
if [[ "$command_name" == s3api ]]; then
  action="$1"; shift
  bucket="" key=""
  while [[ $# -gt 0 ]]; do
    case "$1" in --bucket) bucket="$2"; shift 2 ;; --key) key="$2"; shift 2 ;; *) shift ;; esac
  done
  if [[ "$action" == head-object && -f "$FAKE_R2/$bucket/$key" ]]; then echo '{}'; exit 0; fi
  echo '404 Not Found' >&2; exit 1
fi
action="$1"; source="$2"; target="$3"
[[ "$action" == cp ]]
if [[ "$source" == s3://* ]]; then
  remote="${source#s3://}"
  cp "$FAKE_R2/$remote" "$target"
else
  remote="${target#s3://}"
  mkdir -p "$(dirname "$FAKE_R2/$remote")"
  cp "$source" "$FAKE_R2/$remote"
fi
EOF
chmod +x "$TMP/bin/aws"

export PATH="$TMP/bin:$PATH"
export FAKE_R2="$TMP/remote"
export QUARTUS_R2_AWS="$TMP/bin/aws"
export QUARTUS_R2_ACCOUNT_ID=test-account
export QUARTUS_R2_BUCKET=mister-magik-ci-cache
export QUARTUS_CACHE_DIR="$TMP/cache"
export QUARTUS_HOST_INSTALL_ROOT="$TMP/cache/docker-intelFPGA_lite"
export QUARTUS_INSTALLER_SCRIPT="$TMP/installer.sh"
export QUARTUS_DOCKER_BASE_IMAGE_DIGEST='ubuntu@sha256:test'
export QUARTUS_CACHE_OS=linux
export QUARTUS_CACHE_ARCH=x86_64
export AWS_ACCESS_KEY_ID=test
export AWS_SECRET_ACCESS_KEY=test
export QUARTUS_R2_VERIFY_COMMAND='grep -q "Version 17.0.0 Build 595" "$QUARTUS_HOST_INSTALL_ROOT/version.txt"'

identity1="$($ROOT/scripts/quartus-r2-cache.sh identity)"
identity2="$($ROOT/scripts/quartus-r2-cache.sh identity)"
[[ "$identity1" == "$identity2" && "$identity1" == ci/quartus/v1/linux-x64/17.0.0.595-cyclonev/* ]]
cp "$TMP/installer.sh" "$TMP/installer.original"
printf '\n# identity change\n' >>"$TMP/installer.sh"
[[ "$($ROOT/scripts/quartus-r2-cache.sh identity)" != "$identity1" ]]
mv "$TMP/installer.original" "$TMP/installer.sh"
QUARTUS_CACHE_ARCH=arm64 "$ROOT/scripts/quartus-r2-cache.sh" identity | grep -q '^ci/quartus/v1/linux-arm64/'

set +e
"$ROOT/scripts/quartus-r2-cache.sh" restore >/dev/null 2>"$TMP/miss.err"
status=$?
set -e
[[ "$status" -eq 10 ]]

"$ROOT/scripts/quartus-r2-cache.sh" save >/dev/null
archive="$TMP/remote/$QUARTUS_R2_BUCKET/$identity1/runtime.tar.zst"
manifest="$TMP/remote/$QUARTUS_R2_BUCKET/$identity1/runtime.json"
test -f "$archive" && test -f "$manifest"

rm -rf "$QUARTUS_HOST_INSTALL_ROOT"
"$ROOT/scripts/quartus-r2-cache.sh" restore >/dev/null
grep -q 'Version 17.0.0 Build 595' "$QUARTUS_HOST_INSTALL_ROOT/version.txt"

export QUARTUS_R2_VERIFY_COMMAND='grep -q "Version 18.0" "$QUARTUS_HOST_INSTALL_ROOT/version.txt"'
if "$ROOT/scripts/quartus-r2-cache.sh" restore >/dev/null 2>&1; then
  echo "wrong Quartus runtime version unexpectedly verified" >&2
  exit 1
fi
export QUARTUS_R2_VERIFY_COMMAND='grep -q "Version 17.0.0 Build 595" "$QUARTUS_HOST_INSTALL_ROOT/version.txt"'

printf 'corrupt' >>"$archive"
if "$ROOT/scripts/quartus-r2-cache.sh" restore >/dev/null 2>&1; then
  echo "corrupt archive unexpectedly restored" >&2
  exit 1
fi

rm -f "$manifest"
set +e
"$ROOT/scripts/quartus-r2-cache.sh" restore >/dev/null 2>&1
status=$?
set -e
[[ "$status" -eq 10 ]]

export FAKE_AWS_DENY=1
if "$ROOT/scripts/quartus-r2-cache.sh" restore >/dev/null 2>&1; then
  echo "unauthorized restore unexpectedly succeeded" >&2
  exit 1
fi
unset FAKE_AWS_DENY

echo "Quartus R2 cache tests ok"
