#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUCKET="${QUARTUS_R2_BUCKET:-mister-magik-ci-cache}"
ACCOUNT_ID="${QUARTUS_R2_ACCOUNT_ID:-}"
AWS="${QUARTUS_R2_AWS:-aws}"
CACHE_DIR="${QUARTUS_CACHE_DIR:-$ROOT/build/quartus-lite-17.0}"
INSTALL_ROOT="${QUARTUS_HOST_INSTALL_ROOT:-$CACHE_DIR/docker-intelFPGA_lite}"
INSTALLER_SCRIPT="${QUARTUS_INSTALLER_SCRIPT:-$ROOT/scripts/install-quartus-lite-docker.sh}"
BASE_IMAGE="${QUARTUS_DOCKER_BASE_IMAGE:-ubuntu:18.04}"
VERSION="17.0.0.595"
CYCLONEV_SHA1="2198dedb99866f38d43ff6c029d4bd668e2bbb59"
ARCHIVE_SCHEMA="v1"

usage() {
  echo "usage: scripts/quartus-r2-cache.sh identity|restore|save" >&2
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

normalized_platform() {
  local os arch
  os="${QUARTUS_CACHE_OS:-$(uname -s)}"
  arch="${QUARTUS_CACHE_ARCH:-$(uname -m)}"
  case "${os,,}" in linux) os=linux ;; *) os="${os,,}" ;; esac
  case "$arch" in x86_64|amd64|X64) arch=x64 ;; aarch64|arm64|ARM64) arch=arm64 ;; esac
  printf '%s-%s\n' "$os" "$arch"
}

base_image_digest() {
  if [[ -n "${QUARTUS_DOCKER_BASE_IMAGE_DIGEST:-}" ]]; then
    printf '%s\n' "$QUARTUS_DOCKER_BASE_IMAGE_DIGEST"
    return
  fi
  command -v docker >/dev/null 2>&1 || {
    echo "docker is required to resolve the Quartus base-image digest" >&2
    return 2
  }
  docker pull "$BASE_IMAGE" >/dev/null
  docker image inspect "$BASE_IMAGE" --format '{{index .RepoDigests 0}}'
}

cache_identity() {
  local installer_sha material
  [[ -f "$INSTALLER_SCRIPT" ]] || { echo "missing installer script: $INSTALLER_SCRIPT" >&2; return 2; }
  installer_sha="$(sha256_file "$INSTALLER_SCRIPT")"
  material="schema=$ARCHIVE_SCHEMA
platform=$(normalized_platform)
version=$VERSION
cyclonev_sha1=$CYCLONEV_SHA1
base_image=$(base_image_digest)
installer_sha256=$installer_sha"
  printf '%s' "$material" | sha256sum | awk '{print $1}'
}

object_prefix() {
  printf 'ci/quartus/%s/%s/%s-cyclonev/%s\n' \
    "$ARCHIVE_SCHEMA" "$(normalized_platform)" "$VERSION" "$(cache_identity)"
}

endpoint_args() {
  [[ -n "$ACCOUNT_ID" ]] || { echo "QUARTUS_R2_ACCOUNT_ID is required" >&2; return 2; }
  printf '%s\n' "--endpoint-url" "https://${ACCOUNT_ID}.r2.cloudflarestorage.com" "--region" "auto"
}

require_remote_auth() {
  [[ -n "${AWS_ACCESS_KEY_ID:-}" ]] || { echo "AWS_ACCESS_KEY_ID is required for the private Quartus R2 cache" >&2; return 2; }
  [[ -n "${AWS_SECRET_ACCESS_KEY:-}" ]] || { echo "AWS_SECRET_ACCESS_KEY is required for the private Quartus R2 cache" >&2; return 2; }
  command -v "$AWS" >/dev/null 2>&1 || { echo "$AWS is required for the private Quartus R2 cache" >&2; return 2; }
}

safe_install_root() {
  [[ "$(basename "$INSTALL_ROOT")" == docker-intelFPGA_lite ]] || {
    echo "refusing unsafe Quartus install root: $INSTALL_ROOT" >&2
    return 2
  }
  [[ "$INSTALL_ROOT" != / ]] || return 2
}

verify_runtime() {
  local output
  test -x "$INSTALL_ROOT/17.0/quartus/bin/quartus_sh" || {
    echo "restored Quartus runtime has no quartus_sh" >&2
    return 1
  }
  if [[ -n "${QUARTUS_R2_VERIFY_COMMAND:-}" ]]; then
    bash -c "$QUARTUS_R2_VERIFY_COMMAND"
  else
    output="$(QUARTUS_ACCEPT_EULA=1 "$INSTALLER_SCRIPT")"
    printf '%s\n' "$output"
    grep -Eq 'Version 17\.0(\.0)? Build 595' <<<"$output" || {
      echo "restored Quartus runtime reported the wrong version" >&2
      return 1
    }
  fi
}

restore_cache() {
  local prefix archive_key manifest_key tmp expected actual manifest_identity args=()
  require_remote_auth
  safe_install_root
  mapfile -t args < <(endpoint_args)
  prefix="$(object_prefix)"
  archive_key="$prefix/runtime.tar.zst"
  manifest_key="$prefix/runtime.json"
  tmp="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/quartus-r2-restore.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN

  if ! "$AWS" "${args[@]}" s3api head-object --bucket "$BUCKET" --key "$manifest_key" \
      >"$tmp/head.json" 2>"$tmp/head.err"; then
    if grep -Eqi '404|Not Found|NoSuchKey' "$tmp/head.err"; then
      echo "Quartus R2 cache miss: $manifest_key" >&2
      return 10
    fi
    echo "Quartus R2 lookup failed:" >&2
    sed -n '1,20p' "$tmp/head.err" >&2
    return 1
  fi

  "$AWS" "${args[@]}" s3 cp "s3://$BUCKET/$manifest_key" "$tmp/runtime.json" --only-show-errors
  read -r expected manifest_identity < <(python3 - "$tmp/runtime.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
print(data["sha256"], data["identity"])
PY
  )
  if [[ "$manifest_identity" != "$(cache_identity)" ]]; then
    echo "Quartus R2 manifest identity mismatch" >&2
    return 1
  fi
  "$AWS" "${args[@]}" s3 cp "s3://$BUCKET/$archive_key" "$tmp/runtime.tar.zst" --only-show-errors
  actual="$(sha256_file "$tmp/runtime.tar.zst")"
  if [[ "$actual" != "$expected" ]]; then
    echo "Quartus R2 archive checksum mismatch: expected $expected, got $actual" >&2
    return 1
  fi

  mkdir -p "$(dirname "$INSTALL_ROOT")"
  rm -rf "$INSTALL_ROOT"
  tar --zstd -xf "$tmp/runtime.tar.zst" -C "$(dirname "$INSTALL_ROOT")"
  verify_runtime
  echo "restored Quartus runtime from s3://$BUCKET/$archive_key"
}

save_cache() {
  local prefix archive_key manifest_key tmp sha bytes identity args=()
  require_remote_auth
  safe_install_root
  verify_runtime
  mapfile -t args < <(endpoint_args)
  prefix="$(object_prefix)"
  identity="$(cache_identity)"
  archive_key="$prefix/runtime.tar.zst"
  manifest_key="$prefix/runtime.json"
  tmp="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/quartus-r2-save.XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN

  ZSTD_CLEVEL="${QUARTUS_R2_ZSTD_LEVEL:-3}" ZSTD_NBTHREADS=0 \
    tar --zstd -cf "$tmp/runtime.tar.zst" -C "$(dirname "$INSTALL_ROOT")" "$(basename "$INSTALL_ROOT")"
  sha="$(sha256_file "$tmp/runtime.tar.zst")"
  bytes="$(wc -c <"$tmp/runtime.tar.zst" | tr -d '[:space:]')"
  python3 - "$tmp/runtime.json" "$identity" "$sha" "$bytes" "$VERSION" <<'PY'
import json, sys
path, identity, sha256, size, version = sys.argv[1:]
with open(path, "w", encoding="utf-8") as output:
    json.dump({
        "format": "mister-magik-quartus-r2-cache-v1",
        "identity": identity,
        "quartus_version": version,
        "sha256": sha256,
        "size": int(size),
    }, output, sort_keys=True)
    output.write("\n")
PY

  "$AWS" "${args[@]}" s3 cp "$tmp/runtime.tar.zst" "s3://$BUCKET/$archive_key" --only-show-errors
  "$AWS" "${args[@]}" s3 cp "$tmp/runtime.json" "s3://$BUCKET/$manifest_key" \
    --content-type application/json --only-show-errors
  "$AWS" "${args[@]}" s3api head-object --bucket "$BUCKET" --key "$manifest_key" >/dev/null
  echo "saved Quartus runtime to s3://$BUCKET/$archive_key ($bytes bytes)"
}

case "${1:-}" in
  identity) object_prefix ;;
  restore) restore_cache ;;
  save) save_cache ;;
  -h|--help|"") usage; [[ -n "${1:-}" ]] ;;
  *) usage; exit 2 ;;
esac
