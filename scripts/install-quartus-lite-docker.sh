#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_DIR="${QUARTUS_CACHE_DIR:-$ROOT/build/quartus-lite-17.0}"
IMAGE="${QUARTUS_DOCKER_IMAGE:-mister-magik-quartus-runtime:ubuntu20-amd64}"
BASE_IMAGE="${QUARTUS_DOCKER_BASE_IMAGE:-ubuntu:20.04}"
CONTEXT_DIR="$ROOT/build/quartus-lite-docker-context"
INSTALL_ROOT="${QUARTUS_HOST_INSTALL_ROOT:-$CACHE_DIR/docker-intelFPGA_lite}"
CONTAINERFILE="$CONTEXT_DIR/Dockerfile"

QUARTUS_RUN="QuartusLiteSetup-17.0.0.595-linux.run"
CYCLONEV_QDZ="cyclonev-17.0.0.595.qdz"
UPDATE2_RUN="QuartusSetup-17.0.2.602-linux.run"

usage() {
  cat <<'EOF'
Usage:
  scripts/install-quartus-lite-docker.sh

Builds an amd64 Docker runtime image, then installs Quartus Prime Lite 17.0
plus Cyclone V support into an ignored host cache mounted by that image.

Place these official Altera/Intel downloads in build/quartus-lite-17.0 first:
  QuartusLiteSetup-17.0.0.595-linux.run
  cyclonev-17.0.0.595.qdz

Optional:
  QuartusSetup-17.0.2.602-linux.run

The official download page is:
  https://www.altera.com/downloads/fpga-development-tools/quartus-prime-lite-edition-design-software-version-17-0-linux

Set QUARTUS_ACCEPT_EULA=1 after accepting the Quartus installer terms.
Set QUARTUS_CACHE_DIR, QUARTUS_HOST_INSTALL_ROOT, QUARTUS_DOCKER_IMAGE, or
QUARTUS_DOCKER_BASE_IMAGE to override paths/names.
EOF
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  "")
    ;;
  *)
    echo "unknown argument: $1" >&2
    usage >&2
    exit 2
    ;;
esac

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "missing $path" >&2
    echo "download it from the official Quartus Prime Lite 17.0 Linux page first" >&2
    exit 1
  fi
}

verify_sha1() {
  local path="$1"
  local expected="$2"
  local actual
  actual="$(shasum -a 1 "$path" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "sha1 mismatch for $path" >&2
    echo "expected $expected" >&2
    echo "actual   $actual" >&2
    exit 1
  fi
}

patch_qenv_for_orbstack() {
  local patch="${QUARTUS_PATCH_QENV_FOR_ORBSTACK:-}"
  if [[ -z "$patch" && "${OSTYPE:-}" == darwin* ]]; then
    patch=1
  fi
  if [[ "$patch" != "1" ]]; then
    return 0
  fi
  local qenv="$INSTALL_ROOT/17.0/quartus/adm/qenv.sh"
  [[ -f "$qenv" ]] || return 1
  if grep -q 'orbstack-amd64-emulation' "$qenv"; then
    return 0
  fi
  if [[ ! -f "$qenv.orig" ]]; then
    cp "$qenv" "$qenv.orig"
  fi
  perl -0pi -e 's/# We don.*?fi\n\n##### Determine/# MagiK OrbStack amd64 emulation: Docker reports uname -m=x86_64, but \/proc\/cpuinfo can still expose host ARM flags. Quartus binaries run; bypass only this shell-wrapper SSE probe.\nexport cpumodel="orbstack-amd64-emulation"\n\n##### Determine/s' "$qenv"
  grep -q 'orbstack-amd64-emulation' "$qenv"
}

if [[ "${QUARTUS_ACCEPT_EULA:-}" != "1" ]]; then
  echo "set QUARTUS_ACCEPT_EULA=1 after accepting the Quartus installer terms" >&2
  exit 1
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "missing Docker runtime" >&2
  exit 1
fi

mkdir -p "$CACHE_DIR"
require_file "$CACHE_DIR/$QUARTUS_RUN"
require_file "$CACHE_DIR/$CYCLONEV_QDZ"

verify_sha1 "$CACHE_DIR/$QUARTUS_RUN" "99ccfb15962febceba64de2dc9b28c47e5a3b8df"
verify_sha1 "$CACHE_DIR/$CYCLONEV_QDZ" "2198dedb99866f38d43ff6c029d4bd668e2bbb59"
if [[ -f "$CACHE_DIR/$UPDATE2_RUN" ]]; then
  verify_sha1 "$CACHE_DIR/$UPDATE2_RUN" "cdc0389947ba6d3fb3206ac9840549c9fb38b093"
fi

mkdir -p "$CONTEXT_DIR"
cat > "$CONTAINERFILE" <<EOF
FROM $BASE_IMAGE

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    bash ca-certificates file make perl python3 tar gzip xz-utils locales \
    libc6-i386 lib32stdc++6 lib32z1 \
    libfontconfig1 libfreetype6 libglib2.0-0 libice6 libncurses5 libsm6 \
    libx11-6 libxau6 libxdmcp6 libxext6 libxft2 libxi6 libxrender1 libxt6 \
    libxtst6 \
  && locale-gen en_US.UTF-8 \
  && rm -rf /var/lib/apt/lists/*

ENV LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8 PATH=/opt/intelFPGA_lite/17.0/quartus/bin:$PATH
EOF

docker build --platform linux/amd64 --file "$CONTAINERFILE" --tag "$IMAGE" "$CONTEXT_DIR"

mkdir -p "$INSTALL_ROOT"
if [[ ! -x "$INSTALL_ROOT/17.0/quartus/bin/quartus_sh" ]]; then
  set +e
  docker run --platform linux/amd64 --rm \
    --volume "$CACHE_DIR:/quartus-cache" \
    --volume "$INSTALL_ROOT:/opt/intelFPGA_lite" \
    --workdir /quartus-cache \
    "$IMAGE" \
    bash -lc "chmod +x '$QUARTUS_RUN' && timeout 20m ./'$QUARTUS_RUN' --mode unattended --unattendedmodeui minimal --installdir /opt/intelFPGA_lite/17.0"
  install_status=$?
  set -e

  if [[ "$install_status" -ne 0 && "$install_status" -ne 124 ]]; then
    echo "Quartus installer failed with status $install_status" >&2
    exit "$install_status"
  fi
  if [[ "$install_status" -eq 124 ]]; then
    log="$INSTALL_ROOT/17.0/logs/quartus-17.0.0.595-linux-install.log"
    if [[ ! -f "$log" ]] || ! tail -50 "$log" | grep -q 'Installation completed'; then
      echo "Quartus installer timed out and completion was not found in $log" >&2
      exit 124
    fi
    echo "Quartus installer timed out after completion; continuing with completed install" >&2
  fi
fi

if [[ -f "$CACHE_DIR/$UPDATE2_RUN" ]]; then
  docker run --platform linux/amd64 --rm \
    --volume "$CACHE_DIR:/quartus-cache" \
    --volume "$INSTALL_ROOT:/opt/intelFPGA_lite" \
    --workdir /quartus-cache \
    "$IMAGE" \
    bash -lc "chmod +x '$UPDATE2_RUN' && ./'$UPDATE2_RUN' --mode unattended --unattendedmodeui minimal --installdir /opt/intelFPGA_lite/17.0"
fi

patch_qenv_for_orbstack

docker run --platform linux/amd64 --rm \
  --volume "$INSTALL_ROOT:/opt/intelFPGA_lite:ro" \
  "$IMAGE" \
  quartus_sh --version
echo "$IMAGE"
