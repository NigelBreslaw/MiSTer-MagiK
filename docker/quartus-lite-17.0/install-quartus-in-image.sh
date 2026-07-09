#!/usr/bin/env bash
set -euo pipefail

: "${QUARTUS_RUN:=QuartusLiteSetup-17.0.0.595-linux.run}"
: "${CYCLONEV_QDZ:=cyclonev-17.0.0.595.qdz}"
: "${QUARTUS_SHA1:=99ccfb15962febceba64de2dc9b28c47e5a3b8df}"
: "${CYCLONEV_SHA1:=2198dedb99866f38d43ff6c029d4bd668e2bbb59}"
: "${QUARTUS_INSTALL_TIMEOUT:=20m}"
: "${QUARTUS_INSTALL_HEARTBEAT_SECS:=60}"

INSTALL_DIR=/opt/intelFPGA_lite/17.0
INSTALL_LOG="$INSTALL_DIR/logs/quartus-17.0.0.595-linux-install.log"

echo "${QUARTUS_SHA1}  ${QUARTUS_RUN}" | sha1sum -c -
echo "${CYCLONEV_SHA1}  ${CYCLONEV_QDZ}" | sha1sum -c -
chmod +x "$QUARTUS_RUN"

echo "Installing Quartus Lite 17.0 into $INSTALL_DIR with timeout $QUARTUS_INSTALL_TIMEOUT"
set +e
timeout "$QUARTUS_INSTALL_TIMEOUT" bash -lc '
  set +e
  "./$0" --mode unattended --unattendedmodeui minimal --installdir "$1" &
  installer_pid=$!
  while kill -0 "$installer_pid" 2>/dev/null; do
    echo "quartus installer still running $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    du -sh "$1" 2>/dev/null || true
    sleep "$2"
  done &
  heartbeat_pid=$!
  wait "$installer_pid"
  installer_status=$?
  kill "$heartbeat_pid" 2>/dev/null || true
  wait "$heartbeat_pid" 2>/dev/null || true
  exit "$installer_status"
' "$QUARTUS_RUN" "$INSTALL_DIR" "$QUARTUS_INSTALL_HEARTBEAT_SECS"
install_status=$?
set -e

if [[ "$install_status" -ne 0 && "$install_status" -ne 124 ]]; then
  echo "Quartus installer failed with status $install_status" >&2
  exit "$install_status"
fi

if [[ "$install_status" -eq 124 ]]; then
  if [[ ! -f "$INSTALL_LOG" ]] || ! tail -50 "$INSTALL_LOG" | grep -q 'Installation completed'; then
    echo "Quartus installer timed out and completion was not found in $INSTALL_LOG" >&2
    exit 124
  fi
  echo "Quartus installer timed out after completion; continuing with completed install" >&2
fi

test -x "$INSTALL_DIR/quartus/bin/quartus_sh"
rm -rf /tmp/quartus-installer
