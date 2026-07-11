#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCH="$ROOT/experiments/fpga-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch"
LATCH_RTL="$ROOT/experiments/fpga-vblank-latch/mister_magik_vblank_latch.sv"
OUT_DIR="${MISTER_FPGA_OUT_DIR:-$ROOT/build/fpga-vblank-latch}"
WORK_DIR="${MISTER_MENU_BUILD_DIR:-$OUT_DIR/Menu_MiSTer-vblank-latch-work}"
if [[ -n "${MISTER_MENU_DIR:-}" ]]; then
  MENU_DIR="$MISTER_MENU_DIR"
elif [[ -d "${ROOT}/../Menu_MiSTer" ]]; then
  MENU_DIR="${ROOT}/../Menu_MiSTer"
else
  MENU_DIR="${ROOT}/reference/Menu_MiSTer"
fi
RBF_OUT="$OUT_DIR/menu-magik-vblank-latch.rbf"
META_OUT="$OUT_DIR/menu-magik-vblank-latch.metadata.txt"
LOG_OUT="$OUT_DIR/menu-magik-vblank-latch.build.log"
STRACE_PREFIX="quartus-flow.strace"
QUARTUS_DOCKER_IMAGE="${QUARTUS_DOCKER_IMAGE:-mister-magik-quartus-runtime:ubuntu20-amd64}"
QUARTUS_DOCKER_CPUS="${QUARTUS_DOCKER_CPUS:-8}"
QUARTUS_DOCKER_MEMORY="${QUARTUS_DOCKER_MEMORY:-12g}"
QUARTUS_HOST_INSTALL_ROOT="${QUARTUS_HOST_INSTALL_ROOT:-$ROOT/build/quartus-lite-17.0/docker-intelFPGA_lite}"
APPLY_PATCH="${MISTER_FPGA_APPLY_PATCH:-1}"

usage() {
  cat <<'EOF'
Usage:
  scripts/build-fpga-vblank-latch-core.sh

Builds an experimental Menu_MiSTer RBF with the MiSTer MagiK vblank-latched
framebuffer patch. Set MISTER_MENU_DIR to override the source checkout. The
source checkout is copied to a disposable build workdir before patching.

If quartus_sh is not on PATH, the script will try the Docker runtime image
named by QUARTUS_DOCKER_IMAGE, defaulting to
mister-magik-quartus-runtime:ubuntu20-amd64. Create that runtime plus the
mounted Quartus install with scripts/install-quartus-lite-docker.sh.
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

abs_path() {
  (cd "$1" && pwd -P)
}

if [[ ! -d "$MENU_DIR" ]]; then
  echo "missing Menu_MiSTer checkout: $MENU_DIR" >&2
  echo "set MISTER_MENU_DIR or populate reference/Menu_MiSTer" >&2
  exit 1
fi

MENU_ABS="$(abs_path "$MENU_DIR")"

if [[ ! -f "$MENU_ABS/menu.qsf" || ! -f "$MENU_ABS/sys/sys_top.v" ]]; then
  echo "not a Menu_MiSTer checkout: $MENU_ABS" >&2
  exit 1
fi

QUARTUS_MODE=local
if command -v quartus_sh >/dev/null 2>&1; then
  QUARTUS_CMD="$(command -v quartus_sh)"
elif [[ -x "$QUARTUS_HOST_INSTALL_ROOT/17.0/quartus/bin/quartus_sh" ]] &&
  command -v docker >/dev/null 2>&1 && docker image inspect "$QUARTUS_DOCKER_IMAGE" >/dev/null 2>&1; then
  QUARTUS_MODE=docker
  QUARTUS_CMD="docker:$QUARTUS_DOCKER_IMAGE"
else
  if [[ ! -x "$QUARTUS_HOST_INSTALL_ROOT/17.0/quartus/bin/quartus_sh" ]]; then
    echo "missing mounted Quartus install: $QUARTUS_HOST_INSTALL_ROOT/17.0/quartus/bin/quartus_sh" >&2
  else
    echo "missing Quartus Docker image: $QUARTUS_DOCKER_IMAGE" >&2
  fi
  echo "install it with: QUARTUS_ACCEPT_EULA=1 scripts/install-quartus-lite-docker.sh" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
rm -f "$RBF_OUT" "$META_OUT" "$LOG_OUT" "$OUT_DIR"/"$STRACE_PREFIX"*.log
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"
rsync -a --delete \
  --exclude .git \
  --exclude output_files \
  --exclude db \
  --exclude incremental_db \
  "$MENU_ABS"/ "$WORK_DIR"/
git -C "$WORK_DIR" init -q

case "$APPLY_PATCH" in
  0|false|False|FALSE|no|No|NO)
    echo "skipping MagiK FPGA patch"
    ;;
  *)
    if git -C "$WORK_DIR" apply --recount --check "$PATCH" >/dev/null 2>&1; then
      git -C "$WORK_DIR" apply --recount --whitespace=nowarn "$PATCH"
    elif git -C "$WORK_DIR" apply --recount --reverse --check "$PATCH" >/dev/null 2>&1; then
      echo "patch already applied in work tree"
    else
      echo "patch does not apply cleanly to $MENU_ABS" >&2
      git -C "$WORK_DIR" apply --recount --check "$PATCH"
    fi
    cp "$LATCH_RTL" "$WORK_DIR/sys/mister_magik_vblank_latch.sv"
    printf '\nset_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_vblank_latch.sv\n' >> "$WORK_DIR/menu.qsf"
    ;;
esac

{
  echo "source_dir=$MENU_ABS"
  git -C "$MENU_ABS" rev-parse HEAD 2>/dev/null | sed 's/^/source_commit=/'
  git -C "$MENU_ABS" status --short 2>/dev/null | sed 's/^/source_status=/'
  shasum -a 256 "$PATCH" | awk '{print "patch_sha256="$1}'
  shasum -a 256 "$LATCH_RTL" | awk '{print "latch_rtl_sha256="$1}'
  echo "apply_patch=$APPLY_PATCH"
  echo "work_dir=$WORK_DIR"
  echo "quartus_mode=$QUARTUS_MODE"
  echo "quartus_sh=$QUARTUS_CMD"
  echo "quartus_strace=${QUARTUS_STRACE:-0}"
  echo "quartus_docker_privileged=${QUARTUS_DOCKER_PRIVILEGED:-0}"
  echo "quartus_docker_empty_sys=${QUARTUS_DOCKER_EMPTY_SYS:-0}"
} > "$META_OUT"

build_status=0
set +e
if [[ "$QUARTUS_MODE" = "docker" ]]; then
  docker_security_args=()
  if [[ "${QUARTUS_DOCKER_PRIVILEGED:-}" = "1" ]]; then
    docker_security_args=(--privileged --security-opt seccomp=unconfined)
  fi
  docker_mount_args=()
  if [[ "${QUARTUS_DOCKER_EMPTY_SYS:-}" = "1" ]]; then
    docker_mount_args=(--tmpfs /sys:ro,nosuid,nodev,noexec,mode=0555)
  fi
  docker_quartus_mount_args=(
    --volume "$QUARTUS_HOST_INSTALL_ROOT:/opt/intelFPGA_lite:ro"
    --volume "$WORK_DIR:/work"
  )
  docker_quartus_cmd=(quartus_sh --flow compile menu)
  if [[ "${QUARTUS_STRACE:-}" = "1" ]]; then
    docker_quartus_cmd=(strace -ff -tt -o "/work/$STRACE_PREFIX" quartus_sh --flow compile menu)
  fi
  docker run --platform linux/amd64 --rm \
    "${docker_security_args[@]}" \
    "${docker_mount_args[@]}" \
    --cpus "$QUARTUS_DOCKER_CPUS" \
    --memory "$QUARTUS_DOCKER_MEMORY" \
    "${docker_quartus_mount_args[@]}" \
    --workdir /work \
    "$QUARTUS_DOCKER_IMAGE" \
    "${docker_quartus_cmd[@]}" 2>&1 | tee "$LOG_OUT"
  build_status=$?
else
  (
    cd "$WORK_DIR"
    if [[ "${QUARTUS_STRACE:-}" = "1" ]]; then
      strace -ff -tt -o "$STRACE_PREFIX" quartus_sh --flow compile menu
    else
      quartus_sh --flow compile menu
    fi
  ) 2>&1 | tee "$LOG_OUT"
  build_status=$?
fi
set -e

find "$WORK_DIR" -maxdepth 1 -name "$STRACE_PREFIX*" -print0 |
  while IFS= read -r -d '' trace; do
    cp "$trace" "$OUT_DIR/$(basename "$trace").log"
  done

if [[ "$build_status" -ne 0 ]]; then
  exit "$build_status"
fi

RBF_CANDIDATE="$WORK_DIR/output_files/menu.rbf"
if [[ ! -f "$RBF_CANDIDATE" ]]; then
  RBF_CANDIDATE="$(find "$WORK_DIR" -path '*/output_files/*.rbf' -print -quit)"
fi
if [[ -z "${RBF_CANDIDATE:-}" || ! -f "$RBF_CANDIDATE" ]]; then
  echo "Quartus completed but no RBF was found under $WORK_DIR/output_files" >&2
  exit 1
fi

cp "$RBF_CANDIDATE" "$RBF_OUT"
shasum -a 256 "$RBF_OUT" | awk '{print "rbf_sha256="$1}' >> "$META_OUT"
echo "rbf=$RBF_OUT" >> "$META_OUT"
echo "$RBF_OUT"
