#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCH="$ROOT/mister/platform/fpga/menu-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch"
LATCH_RTL="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_vblank_latch.sv"
LATCH_BRIDGE="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_latch_sys_top_bridge.sv"
BOOTSTRAP_BLACK_RTL="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_bootstrap_black.sv"
LATCH_PROTOCOL="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_latch_protocol.svh"
VIDEO_DIAGNOSTICS_CONTROL="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_control.sv"
VIDEO_DIAGNOSTICS_AVALON="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_avalon.sv"
VIDEO_DIAGNOSTICS_OUTPUT="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_output.sv"
VIDEO_DIAGNOSTICS_PROTOCOL="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics_protocol.svh"
VIDEO_DIAGNOSTICS_SDC="$ROOT/mister/platform/fpga/menu-vblank-latch/mister_magik_video_diagnostics.sdc"
TIMING_REPORT_TCL="$ROOT/mister/platform/fpga/menu-vblank-latch/report_top_timing.tcl"
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
TIMING_LOG_OUT="$OUT_DIR/menu-magik-vblank-latch.top-timing.log"
STRACE_PREFIX="quartus-flow.strace"
QUARTUS_DOCKER_IMAGE="${QUARTUS_DOCKER_IMAGE:-mister-magik-quartus-runtime:ubuntu20-amd64}"
QUARTUS_DOCKER_CPUS="${QUARTUS_DOCKER_CPUS:-8}"
QUARTUS_DOCKER_MEMORY="${QUARTUS_DOCKER_MEMORY:-12g}"
QUARTUS_HOST_INSTALL_ROOT="${QUARTUS_HOST_INSTALL_ROOT:-$ROOT/build/quartus-lite-17.0/docker-intelFPGA_lite}"
APPLY_PATCH="${MISTER_FPGA_APPLY_PATCH:-1}"
BUILD_DATE="${MISTER_FPGA_BUILD_DATE:-$(git -C "$ROOT" show -s --format=%cd --date=format:%y%m%d HEAD)}"
QUALIFIED_MAGIK_REVISION="${MISTER_FPGA_QUALIFIED_MAGIK_REVISION:-$(git -C "$ROOT" rev-parse HEAD)}"
COMPONENT_INPUT_SHA256="${MISTER_FPGA_COMPONENT_INPUT_SHA256:-}"
COMPONENT_REVISION="${MISTER_FPGA_COMPONENT_REVISION:-}"
SYNTHESIS_INPUT_SHA256="${MISTER_FPGA_SYNTHESIS_INPUT_SHA256:-}"
QUARTUS_SEED="${MISTER_FPGA_QUARTUS_SEED:-}"
PLATFORM_CONTRACT="$ROOT/mister/platform/kernel/scanout-slots/mister_magik_scanout_platform.h"

usage() {
  cat <<'EOF'
Usage:
  scripts/build-fpga-vblank-latch-core.sh

Builds the production Menu_MiSTer RBF with the MiSTer MagiK vblank-latched
framebuffer patch. Set MISTER_MENU_DIR to override the source checkout. The
source checkout is copied to a disposable build workdir before patching.

RBF synthesis is supported only inside the repository's GitHub Actions
workflow. Local invocation is rejected before any output directory is created.
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

if [[ "${GITHUB_ACTIONS:-}" != "true" ]]; then
  echo "RBF builds are GitHub Actions only; run the Build MiSTer MagiK Platform workflow" >&2
  exit 1
fi

abs_path() {
  (cd "$1" && pwd -P)
}

if [[ ! -d "$MENU_DIR" ]]; then
  echo "missing Menu_MiSTer checkout: $MENU_DIR" >&2
  echo "set MISTER_MENU_DIR or populate reference/Menu_MiSTer" >&2
  exit 1
fi
if [[ ! -f "$PLATFORM_CONTRACT" ]]; then
  echo "missing scanout platform contract: $PLATFORM_CONTRACT" >&2
  exit 1
fi
if [[ ! -f "$LATCH_PROTOCOL" ]]; then
  echo "missing latch protocol header: $LATCH_PROTOCOL" >&2
  exit 1
fi
if [[ ! -f "$LATCH_BRIDGE" ]]; then
  echo "missing latch sys_top bridge: $LATCH_BRIDGE" >&2
  exit 1
fi
if [[ ! -f "$BOOTSTRAP_BLACK_RTL" ]]; then
  echo "missing bootstrap black RTL: $BOOTSTRAP_BLACK_RTL" >&2
  exit 1
fi
if [[ ! -f "$TIMING_REPORT_TCL" ]]; then
  echo "missing top timing report script: $TIMING_REPORT_TCL" >&2
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
rm -f "$RBF_OUT" "$META_OUT" "$LOG_OUT" "$TIMING_LOG_OUT" "$OUT_DIR"/"$STRACE_PREFIX"*.log
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR"
rsync -a --delete \
  --exclude .git \
  --exclude output_files \
  --exclude db \
  --exclude incremental_db \
  "$MENU_ABS"/ "$WORK_DIR"/
git -C "$WORK_DIR" init -q
cp "$TIMING_REPORT_TCL" "$WORK_DIR/mister_magik_report_top_timing.tcl"

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
    cp "$LATCH_BRIDGE" "$WORK_DIR/sys/mister_magik_latch_sys_top_bridge.sv"
    cp "$BOOTSTRAP_BLACK_RTL" "$WORK_DIR/sys/mister_magik_bootstrap_black.sv"
    cp "$LATCH_PROTOCOL" "$WORK_DIR/sys/mister_magik_latch_protocol.svh"
    cp "$VIDEO_DIAGNOSTICS_CONTROL" "$WORK_DIR/sys/mister_magik_video_diagnostics_control.sv"
    cp "$VIDEO_DIAGNOSTICS_AVALON" "$WORK_DIR/sys/mister_magik_video_diagnostics_avalon.sv"
    cp "$VIDEO_DIAGNOSTICS_OUTPUT" "$WORK_DIR/sys/mister_magik_video_diagnostics_output.sv"
    cp "$VIDEO_DIAGNOSTICS_PROTOCOL" "$WORK_DIR/sys/mister_magik_video_diagnostics_protocol.svh"
	cp "$VIDEO_DIAGNOSTICS_SDC" "$WORK_DIR/sys/mister_magik_video_diagnostics.sdc"
    printf '\nset_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_vblank_latch.sv\nset_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_latch_sys_top_bridge.sv\nset_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_bootstrap_black.sv\n' >> "$WORK_DIR/menu.qsf"
    printf 'set_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_control.sv\n' >> "$WORK_DIR/menu.qsf"
    printf 'set_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_avalon.sv\n' >> "$WORK_DIR/menu.qsf"
    printf 'set_global_assignment -name SYSTEMVERILOG_FILE sys/mister_magik_video_diagnostics_output.sv\n' >> "$WORK_DIR/menu.qsf"
	printf 'set_global_assignment -name SDC_FILE sys/mister_magik_video_diagnostics.sdc\n' >> "$WORK_DIR/menu.qsf"
    ;;
esac
if [[ ! "$BUILD_DATE" =~ ^[0-9]{6}$ ]]; then
  echo "MISTER_FPGA_BUILD_DATE must be a six-digit YYMMDD value" >&2
  exit 2
fi
if [[ -n "$COMPONENT_INPUT_SHA256" && ! "$COMPONENT_INPUT_SHA256" =~ ^[0-9a-f]{64}$ ]]; then
  echo "MISTER_FPGA_COMPONENT_INPUT_SHA256 must be a SHA-256 value" >&2
  exit 1
fi
if [[ -n "$COMPONENT_REVISION" && ! "$COMPONENT_REVISION" =~ ^[0-9a-f]{40}$ ]]; then
  echo "MISTER_FPGA_COMPONENT_REVISION must be a full commit SHA" >&2
  exit 1
fi
if [[ -n "$SYNTHESIS_INPUT_SHA256" && ! "$SYNTHESIS_INPUT_SHA256" =~ ^[0-9a-f]{64}$ ]]; then
  echo "MISTER_FPGA_SYNTHESIS_INPUT_SHA256 must be a SHA-256 value" >&2
  exit 1
fi
if [[ -n "$QUARTUS_SEED" && ! "$QUARTUS_SEED" =~ ^[1-9][0-9]*$ ]]; then
  echo "MISTER_FPGA_QUARTUS_SEED must be a positive integer" >&2
  exit 1
fi
if [[ -n "$QUARTUS_SEED" ]]; then
  python3 - "$WORK_DIR/menu.qsf" "$QUARTUS_SEED" <<'PY'
from pathlib import Path
import re
import sys

path = Path(sys.argv[1])
seed = sys.argv[2]
source = path.read_text()
updated, count = re.subn(
    r"^(?:-?set_global_assignment -name SEED) [0-9]+$",
    rf"set_global_assignment -name SEED {seed}",
    source,
    count=1,
    flags=re.MULTILINE,
)
if count != 1:
    raise SystemExit("failed to set Quartus fitter seed")
path.write_text(updated)
PY
fi
python3 - "$WORK_DIR/sys/build_id.tcl" "$BUILD_DATE" <<'PY'
from pathlib import Path
import re
import sys

path = Path(sys.argv[1])
date = sys.argv[2]
source = path.read_text()
updated, count = re.subn(
    r'set buildDate "`define BUILD_DATE .*?"$',
    f'set buildDate "`define BUILD_DATE \\"{date}\\""',
    source,
    count=1,
    flags=re.MULTILINE,
)
if count != 1:
    raise SystemExit("failed to pin Menu build_id.tcl timestamp")
path.write_text(updated)
PY

{
  echo "format=mister-magik-fpga-release-v2"
  shasum -a 256 "$PLATFORM_CONTRACT" | awk '{print "platform_contract_sha256="$1}'
  echo "magik_commit=$QUALIFIED_MAGIK_REVISION"
  if [[ -n "$COMPONENT_INPUT_SHA256" ]]; then
    echo "component_input_sha256=$COMPONENT_INPUT_SHA256"
    echo "component_revision=$COMPONENT_REVISION"
  fi
  if [[ -n "$SYNTHESIS_INPUT_SHA256" ]]; then
    echo "synthesis_input_sha256=$SYNTHESIS_INPUT_SHA256"
  fi
  git -C "$ROOT" rev-parse HEAD 2>/dev/null | sed 's/^/builder_commit=/'
  git -C "$ROOT" status --short --untracked-files=no 2>/dev/null | sed 's/^/magik_status=/'
  echo "source_dir=$MENU_ABS"
  git -C "$MENU_ABS" rev-parse HEAD 2>/dev/null | sed 's/^/source_commit=/'
  git -C "$MENU_ABS" status --short 2>/dev/null | sed 's/^/source_status=/'
  shasum -a 256 "$PATCH" | awk '{print "patch_sha256="$1}'
  shasum -a 256 "$LATCH_RTL" | awk '{print "latch_rtl_sha256="$1}'
  shasum -a 256 "$LATCH_BRIDGE" | awk '{print "latch_bridge_sha256="$1}'
  shasum -a 256 "$LATCH_PROTOCOL" | awk '{print "latch_protocol_sha256="$1}'
  python3 -c 'import re,sys; source=open(sys.argv[1]).read(); match=re.search(r"MAGIK_FBUF_PROTOCOL_VERSION\s*=\s*16.d(\d+)", source); assert match; print("latch_protocol_version=" + match.group(1))' "$LATCH_PROTOCOL"
  python3 -c 'import re,sys; source=open(sys.argv[1]).read(); match=re.search(r"MAGIK_FBUF_CAPS_FLAGS\s*=\s*16.h([0-9A-Fa-f]+)", source); assert match; print("latch_capability_mask=0x" + match.group(1).lower())' "$LATCH_PROTOCOL"
  echo "apply_patch=$APPLY_PATCH"
  echo "build_date=$BUILD_DATE"
  echo "work_dir=$WORK_DIR"
  echo "quartus_mode=$QUARTUS_MODE"
  echo "quartus_sh=$QUARTUS_CMD"
  echo "quartus_strace=${QUARTUS_STRACE:-0}"
  echo "quartus_docker_privileged=${QUARTUS_DOCKER_PRIVILEGED:-0}"
  echo "quartus_docker_empty_sys=${QUARTUS_DOCKER_EMPTY_SYS:-0}"
  awk '/^-?set_global_assignment -name SEED / {print "quartus_seed="$NF}' "$WORK_DIR/menu.qsf" | tail -1
  if [[ -n "${GITHUB_SERVER_URL:-}" && -n "${GITHUB_REPOSITORY:-}" && -n "${GITHUB_RUN_ID:-}" ]]; then
    echo "workflow_url=$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
  else
    echo "workflow_url=local"
  fi
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

if [[ "$QUARTUS_MODE" = "docker" ]]; then
  docker run --platform linux/amd64 --rm \
    "${docker_security_args[@]}" \
    "${docker_mount_args[@]}" \
    --cpus "$QUARTUS_DOCKER_CPUS" \
    --memory "$QUARTUS_DOCKER_MEMORY" \
    "${docker_quartus_mount_args[@]}" \
    --workdir /work \
    "$QUARTUS_DOCKER_IMAGE" \
    quartus_sta -t mister_magik_report_top_timing.tcl 2>&1 | tee "$TIMING_LOG_OUT"
else
  (
    cd "$WORK_DIR"
    quartus_sta -t mister_magik_report_top_timing.tcl
  ) 2>&1 | tee "$TIMING_LOG_OUT"
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
echo "rbf_file=$(basename "$RBF_OUT")" >> "$META_OUT"
quartus_version="$(sed -n 's/.*Version \([0-9][^ ]* Build [0-9][^ ]*\).*/\1/p' "$LOG_OUT" | head -1)"
if [[ -z "$quartus_version" ]]; then
  echo "Quartus version was not found in $LOG_OUT" >&2
  exit 1
fi
echo "quartus_version=$quartus_version" >> "$META_OUT"
rm -rf "$OUT_DIR/reports"
mkdir -p "$OUT_DIR/reports"
find "$WORK_DIR/output_files" -maxdepth 1 -type f \( -name '*.rpt' -o -name '*.summary' \) -exec cp {} "$OUT_DIR/reports/" \;
find "$OUT_DIR/reports" -type f -print0 | sort -z | while IFS= read -r -d '' report; do
  relative="reports/$(basename "$report")"
  hash="$(shasum -a 256 "$report" | awk '{print $1}')"
  echo "report_sha256.$relative=$hash" >> "$META_OUT"
done
echo "$RBF_OUT"
