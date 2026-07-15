#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

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
Usage: scripts/mister-shutdown-trace.sh <install|install-deep|remove|status|log|bypass-s99user-install|bypass-s99user-remove|bypass-s99user-status>

Installs reversible wrappers for these /etc/inittab shutdown actions:
  ::shutdown:/etc/init.d/rcK
  ::shutdown:/sbin/swapoff -a
  ::shutdown:/bin/umount -a -r

install keeps rcK as one timed step.
install-deep times each /etc/init.d/S??* stop inside rcK, then swapoff and umount.
bypass-s99user-* is a separate reversible experiment for isolating user hooks.

Logs are appended to:
  $REMOTE_LOG
EOF
}

install_trace() {
  local mode="${1:-shallow}"
  local rcK_step="rcK"
  if [[ "$mode" == "deep" ]]; then
    rcK_step="rcK-deep"
  fi
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

elapsed() {
  awk "BEGIN { printf \"%.2f\", ($2) - ($1) }" 2>/dev/null || echo "?"
}

run_step() {
  name="$1"
  shift
  step_start="$(stamp)"
  log "start step=$name cmd=$*"
  "$@"
  rc=$?
  step_end="$(stamp)"
  log "done step=$name rc=$rc elapsed_s=$(elapsed "$step_start" "$step_end")"
  return "$rc"
}

run_service_stop() {
  service="$1"
  [ ! -f "$service" ] && return 0
  service_start="$(stamp)"
  log "start step=rcK.service service=$service"
  case "$service" in
    *.sh)
      (
        trap - INT QUIT TSTP
        set stop
        . "$service"
      )
      ;;
    *)
      "$service" stop
      ;;
  esac
  rc=$?
  service_end="$(stamp)"
  log "done step=rcK.service service=$service rc=$rc elapsed_s=$(elapsed "$service_start" "$service_end")"
  return "$rc"
}

run_rcK_deep() {
  rck_start="$(stamp)"
  log "start step=rcK-deep cmd=/etc/init.d/S??*"
  rc=0
  for service in $(ls -r /etc/init.d/S??* 2>/dev/null); do
    run_service_stop "$service" || rc=$?
  done
  rck_end="$(stamp)"
  log "done step=rcK-deep rc=$rc elapsed_s=$(elapsed "$rck_start" "$rck_end")"
  return "$rc"
}

case "${1:-}" in
  rcK)
    run_step rcK /etc/init.d/rcK
    ;;
  rcK-deep)
    run_rcK_deep
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
  /^::shutdown:\/etc\/init.d\/rcK$/ { print \"::shutdown:$REMOTE_SCRIPT $rcK_step\"; next }
  /^::shutdown:\/sbin\/swapoff -a$/ { print \"::shutdown:$REMOTE_SCRIPT swapoff\"; next }
  /^::shutdown:\/bin\/umount -a -r$/ { print \"::shutdown:$REMOTE_SCRIPT umount\"; next }
  /^::shutdown:\/media\/fat\/mister-magik\/shutdown-trace.sh rcK$/ { print \"::shutdown:$REMOTE_SCRIPT $rcK_step\"; next }
  /^::shutdown:\/media\/fat\/mister-magik\/shutdown-trace.sh rcK-deep$/ { print \"::shutdown:$REMOTE_SCRIPT $rcK_step\"; next }
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
    -e 's#^::shutdown:/media/fat/mister-magik/shutdown-trace.sh rcK-deep\$#::shutdown:/etc/init.d/rcK#' \
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

bypass_s99user_install() {
  "$MISTER" run "
set -eu
mount -o remount,rw / 2>/dev/null || true
if [ -f /etc/init.d/S99user ] && [ ! -f /etc/init.d/disabled-S99user.shutdown-trace ]; then
  mv /etc/init.d/S99user /etc/init.d/disabled-S99user.shutdown-trace
fi
cat >/etc/init.d/S99user <<'EOS'
#!/bin/sh
case \"\$1\" in
  start|stop|restart)
    echo \"S99user bypassed for MiSTer MagiK shutdown timing experiment\"
    ;;
  *)
    echo \"Usage: \$0 {start|stop|restart}\"
    exit 1
    ;;
esac
exit 0
EOS
chmod 755 /etc/init.d/S99user
sync
mount -o remount,ro / 2>/dev/null || true
echo s99user_bypass=installed
ls -l /etc/init.d/S99user /etc/init.d/disabled-S99user.shutdown-trace 2>/dev/null || true
"
}

bypass_s99user_remove() {
  "$MISTER" run "
set -eu
mount -o remount,rw / 2>/dev/null || true
if [ -f /etc/init.d/disabled-S99user.shutdown-trace ]; then
  mv /etc/init.d/disabled-S99user.shutdown-trace /etc/init.d/S99user
  echo s99user_bypass=removed
else
  echo s99user_bypass=not-installed
fi
sync
mount -o remount,ro / 2>/dev/null || true
ls -l /etc/init.d/S99user 2>/dev/null || true
"
}

bypass_s99user_status() {
  "$MISTER" run "
if [ -f /etc/init.d/disabled-S99user.shutdown-trace ]; then
  echo s99user_bypass=installed
else
  echo s99user_bypass=not-installed
fi
ls -l /etc/init.d/S99user /etc/init.d/disabled-S99user.shutdown-trace 2>/dev/null || true
"
}

case "${1:-}" in
  install) install_trace shallow ;;
  install-deep) install_trace deep ;;
  remove) remove_trace ;;
  status) status_trace ;;
  log) log_trace ;;
  bypass-s99user-install) bypass_s99user_install ;;
  bypass-s99user-remove) bypass_s99user_remove ;;
  bypass-s99user-status) bypass_s99user_status ;;
  -h|--help|"") usage ;;
  *) echo "unknown action: $1" >&2; usage >&2; exit 2 ;;
esac
