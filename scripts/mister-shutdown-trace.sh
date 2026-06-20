#!/usr/bin/env bash
# Install/remove persistent shutdown timing breadcrumbs around BusyBox init.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
TRACE_DIR="/media/fat/mister-magik/shutdown-trace"
REMOTE_SCRIPT="/media/fat/mister-magik/shutdown-trace.sh"
REMOTE_INITTAB_BAK="$TRACE_DIR/inittab.bak"
REMOTE_MARKER="$TRACE_DIR/installed"
REMOTE_LOG="/media/fat/mister-magik/bootlogs/shutdown-trace.log"

usage() {
  cat <<EOF
Usage: scripts/mister-shutdown-trace.sh <install|remove|status|log>

Installs reversible wrappers for these /etc/inittab shutdown actions:
  ::shutdown:/etc/init.d/rcK
  ::shutdown:/sbin/swapoff -a
  ::shutdown:/bin/umount -a -r

Logs are appended to:
  $REMOTE_LOG
EOF
}

install_trace() {
  local tmp
  tmp="$(mktemp)"
  cat >"$tmp" <<'EOS'
#!/bin/sh
LOG=/media/fat/mister-magik/bootlogs/shutdown-trace.log

stamp() {
  awk '{ printf "%.2f", $1 }' /proc/uptime 2>/dev/null || echo 0
}

log() {
  mkdir -p /media/fat/mister-magik/bootlogs 2>/dev/null || true
  echo "$(stamp) shutdown-trace $*" >>"$LOG" 2>/dev/null || true
  sync
}

run_step() {
  name="$1"
  shift
  log "start step=$name cmd=$*"
  "$@"
  rc=$?
  log "done step=$name rc=$rc"
  return "$rc"
}

case "${1:-}" in
  rcK)
    run_step rcK /etc/init.d/rcK
    ;;
  swapoff)
    run_step swapoff /sbin/swapoff -a
    ;;
  umount)
    run_step umount /bin/umount -a -r
    ;;
  *)
    log "error unknown_step=${1:-missing}"
    exit 2
    ;;
esac
EOS
  "$MISTER" put "$tmp" /tmp/shutdown-trace.sh >/dev/null
  rm -f "$tmp"
  "$MISTER" run "
set -eu
mkdir -p '$TRACE_DIR' /media/fat/mister-magik/bootlogs
mount -o remount,rw / 2>/dev/null || true
if [ ! -f '$REMOTE_INITTAB_BAK' ]; then
  cp /etc/inittab '$REMOTE_INITTAB_BAK'
fi
cp /tmp/shutdown-trace.sh '$REMOTE_SCRIPT'
chmod 755 '$REMOTE_SCRIPT'
tmp=/tmp/inittab.shutdown-trace.\$\$
awk '
  /^::shutdown:\/etc\/init.d\/rcK$/ { print \"::shutdown:$REMOTE_SCRIPT rcK\"; next }
  /^::shutdown:\/sbin\/swapoff -a$/ { print \"::shutdown:$REMOTE_SCRIPT swapoff\"; next }
  /^::shutdown:\/bin\/umount -a -r$/ { print \"::shutdown:$REMOTE_SCRIPT umount\"; next }
  /^::shutdown:\/media\/fat\/mister-magik\/shutdown-trace.sh rcK$/ { print; next }
  /^::shutdown:\/media\/fat\/mister-magik\/shutdown-trace.sh swapoff$/ { print; next }
  /^::shutdown:\/media\/fat\/mister-magik\/shutdown-trace.sh umount$/ { print; next }
  { print }
' /etc/inittab > \"\$tmp\"
cp \"\$tmp\" /etc/inittab
echo installed >'$REMOTE_MARKER'
sync
mount -o remount,ro / 2>/dev/null || true
echo shutdown_trace=installed
grep shutdown /etc/inittab
"
}

remove_trace() {
  "$MISTER" run "
set -eu
mount -o remount,rw / 2>/dev/null || true
if [ -f '$REMOTE_INITTAB_BAK' ]; then
  cp '$REMOTE_INITTAB_BAK' /etc/inittab
else
  tmp=/tmp/inittab.shutdown-trace-remove.\$\$
  sed \
    -e 's#^::shutdown:/media/fat/mister-magik/shutdown-trace.sh rcK\$#::shutdown:/etc/init.d/rcK#' \
    -e 's#^::shutdown:/media/fat/mister-magik/shutdown-trace.sh swapoff\$#::shutdown:/sbin/swapoff -a#' \
    -e 's#^::shutdown:/media/fat/mister-magik/shutdown-trace.sh umount\$#::shutdown:/bin/umount -a -r#' \
    /etc/inittab > \"\$tmp\"
  cp \"\$tmp\" /etc/inittab
fi
rm -f '$REMOTE_MARKER'
sync
mount -o remount,ro / 2>/dev/null || true
echo shutdown_trace=removed
grep shutdown /etc/inittab
"
}

status_trace() {
  "$MISTER" run "
if [ -f '$REMOTE_MARKER' ]; then
  echo shutdown_trace=installed
else
  echo shutdown_trace=not-installed
fi
grep shutdown /etc/inittab
ls -l '$REMOTE_SCRIPT' '$REMOTE_INITTAB_BAK' 2>/dev/null || true
"
}

log_trace() {
  "$MISTER" run "cat '$REMOTE_LOG' 2>/dev/null || true"
}

case "${1:-}" in
  install) install_trace ;;
  remove) remove_trace ;;
  status) status_trace ;;
  log) log_trace ;;
  -h|--help|"") usage ;;
  *) echo "unknown action: $1" >&2; usage >&2; exit 2 ;;
esac
