#!/usr/bin/env bash
# Build Main_MiSTer inside the project devcontainer toolchain.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="${MISTER_MAIN_DOCKER_IMAGE:-mister-magik-main-builder:arm-gcc-10.2}"

if ! command -v docker >/dev/null 2>&1; then
  echo "ERROR: docker is not installed or not on PATH." >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  cat >&2 <<'EOF'
ERROR: docker is installed but the daemon is not reachable.

On this machine Docker is expected to come from OrbStack or Docker Desktop.
Start/fix that runtime, then retry:

  main-mister/build-docker.sh

EOF
  docker info >/dev/null
fi

echo "==> Building Docker image: $IMAGE"
docker build \
  --platform linux/amd64 \
  -t "$IMAGE" \
  -f "$HERE/.devcontainer/Dockerfile" \
  "$HERE/.devcontainer"

echo "==> Building Main_MiSTer in Docker"
docker run --rm \
  --platform linux/amd64 \
  -v "$HERE:/src" \
  -w /src \
  "$IMAGE" \
  bash -lc 'GCC_BIN="$(echo /usr/local/bin/gcc-arm-*/bin)"; export PATH="$GCC_BIN:$PATH"; make "$@"' \
  bash "$@"
