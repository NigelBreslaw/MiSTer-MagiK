#!/usr/bin/env bash
# Install/remove the standalone MiSTer MagiK boot/network agent.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-agent"
REMOTE_SCRIPT="/etc/init.d/S00magik-agent"
REMOTE_TOKEN="/media/fat/mister-magik/agent.token"
LOCAL_TOKEN="$HERE/build/mister-agent.token"
FASTNET_SCRIPT="/etc/init.d/S00fastnet"
FASTNET_DISABLED="/etc/init.d/disabled-S00fastnet.magik-agent"
MARKER="/media/fat/mister-magik/S00magik-agent.experiment"

usage() {
  cat <<EOF
Usage: scripts/mister-magik-agent.sh <build|install|remove|status|log>

Installs a compiled early network agent as /etc/init.d/S00magik-agent and
temporarily disables the shell FastNet service so timings are isolated.

The TCP control token lives at /media/fat/mister-magik/agent.token. A local
copy is kept at build/mister-agent.token for scripts/mister agent commands.

SD-card recovery if SSH is lost: delete /etc/init.d/S00magik-agent. If needed,
rename /etc/init.d/disabled-S00fastnet.magik-agent back to /etc/init.d/S00fastnet.
EOF
}

build_agent() {
  "$HERE/scripts/build-mister-agent.sh"
}

ensure_token() {
  mkdir -p "$HERE/build"
  if [ ! -s "$LOCAL_TOKEN" ]; then
    "$MISTER" get "$REMOTE_TOKEN" "$LOCAL_TOKEN" >/dev/null 2>&1 || true
  fi
  if [ ! -s "$LOCAL_TOKEN" ]; then
    if command -v openssl >/dev/null 2>&1; then
      openssl rand -hex 32 >"$LOCAL_TOKEN"
    elif command -v uuidgen >/dev/null 2>&1; then
      {
        uuidgen
        uuidgen
      } | tr -d '-\n' >"$LOCAL_TOKEN"
      printf '\n' >>"$LOCAL_TOKEN"
    else
      printf '%s-%s-%s\n' "$(date +%s)" "$$" "$RANDOM" >"$LOCAL_TOKEN"
    fi
    chmod 600 "$LOCAL_TOKEN" 2>/dev/null || true
  fi
}

install_agent() {
  local bin
  bin="$(build_agent)"
  ensure_token
  "$MISTER" run "mkdir -p /media/fat/mister-magik" >/dev/null
  "$MISTER" put "$bin" "$REMOTE_BIN.upload" >/dev/null
  "$MISTER" put "$LOCAL_TOKEN" "$REMOTE_TOKEN.upload" >/dev/null
  "$MISTER" run "
set -eu
mv '$REMOTE_BIN.upload' '$REMOTE_BIN'
chmod 755 '$REMOTE_BIN'
if [ ! -s '$REMOTE_TOKEN' ]; then
  mv '$REMOTE_TOKEN.upload' '$REMOTE_TOKEN'
  chmod 600 '$REMOTE_TOKEN' || true
else
  rm -f '$REMOTE_TOKEN.upload'
fi
mount -o remount,rw /
if [ -f '$FASTNET_SCRIPT' ] && [ ! -f '$FASTNET_DISABLED' ]; then
  mv '$FASTNET_SCRIPT' '$FASTNET_DISABLED'
fi
cat >'$REMOTE_SCRIPT' <<'EOS'
#!/bin/sh
stop_agent() {
  pids=\"\$(pidof mister-magik-agent 2>/dev/null || true)\"
  if [ -z \"\$pids\" ]; then
    return 0
  fi
  kill \$pids 2>/dev/null || true
  i=0
  while [ \"\$i\" -lt 20 ]; do
    sleep 0.1
    if ! pidof mister-magik-agent >/dev/null 2>&1; then
      return 0
    fi
    i=\$((i + 1))
  done
  kill -9 \$(pidof mister-magik-agent 2>/dev/null || true) 2>/dev/null || true
}
case \"\$1\" in
  start)
    /media/fat/mister-magik/mister-magik-agent net-boot >/tmp/mister-magik-agent.boot.out 2>&1 &
    ;;
  stop)
    stop_agent
    ;;
  restart)
    stop_agent
    /media/fat/mister-magik/mister-magik-agent net-boot >/tmp/mister-magik-agent.boot.out 2>&1 &
    ;;
  *)
    echo \"Usage: \$0 {start|stop|restart}\"
    exit 1
    ;;
esac
exit 0
EOS
chmod 755 '$REMOTE_SCRIPT'
echo installed >'$MARKER'
'$REMOTE_SCRIPT' restart
sync
mount -o remount,ro / || true
echo magik_agent=installed
if [ -s '$REMOTE_TOKEN' ]; then echo agent_token=installed; fi
ls -l '$REMOTE_SCRIPT' '$REMOTE_BIN'
echo magik_agent_pid=\$(pidof mister-magik-agent 2>/dev/null || true)
"
  "$MISTER" get "$REMOTE_TOKEN" "$LOCAL_TOKEN" >/dev/null 2>&1 || true
}

remove_agent() {
  "$MISTER" run "
set -eu
mount -o remount,rw /
if [ -x '$REMOTE_SCRIPT' ]; then '$REMOTE_SCRIPT' stop || true; else kill \$(pidof mister-magik-agent 2>/dev/null || true) 2>/dev/null || true; fi
rm -f '$REMOTE_SCRIPT'
if [ -f '$FASTNET_DISABLED' ]; then
  mv '$FASTNET_DISABLED' '$FASTNET_SCRIPT'
fi
rm -f '$MARKER'
sync
mount -o remount,ro / || true
echo magik_agent=removed
"
}

status_agent() {
  "$MISTER" run "
if [ -f '$REMOTE_SCRIPT' ]; then
  echo magik_agent=installed
  ls -l '$REMOTE_SCRIPT'
else
  echo magik_agent=not-installed
fi
if [ -f '$FASTNET_SCRIPT' ]; then
  echo fastnet=enabled
fi
if [ -f '$FASTNET_DISABLED' ]; then
  echo fastnet=disabled-for-agent
fi
if [ -x '$REMOTE_BIN' ]; then
  ls -l '$REMOTE_BIN'
fi
if [ -s '$REMOTE_TOKEN' ]; then
  echo agent_token=installed
else
  echo agent_token=missing
fi
echo magik_agent_pid=\$(pidof mister-magik-agent 2>/dev/null || true)
"
}

log_agent() {
  "$MISTER" run "cat /tmp/mister-magik-agent.log 2>/dev/null || true"
}

case "${1:-}" in
  build) build_agent ;;
  install) install_agent ;;
  remove) remove_agent ;;
  status) status_agent ;;
  log) log_agent ;;
  -h|--help|"") usage ;;
  *) echo "unknown action: $1" >&2; usage >&2; exit 2 ;;
esac
