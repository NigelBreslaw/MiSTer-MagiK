#!/usr/bin/env bash
# Swap in a complete Zaparoo or MiSTer MagiK INI file and reboot the MiSTer.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"

usage() {
  cat <<'EOF'
Usage:
  scripts/switch-ui.sh -zaparoo
  scripts/switch-ui.sh -magik

Requires these complete, pre-existing files on the MiSTer:
  /media/fat/MiSTer.ini.zaparoo
  /media/fat/MiSTer.ini.magik

Atomically copies the selected file over /media/fat/MiSTer.ini, syncs it,
and performs a normal Linux reboot. The two source files are never modified.
EOF
}

if [[ $# -ne 1 ]]; then
  usage >&2
  exit 2
fi

case "$1" in
  -zaparoo)
    label="Zaparoo"
    source_ini="/media/fat/MiSTer.ini.zaparoo"
    ;;
  -magik)
    label="MiSTer MagiK"
    source_ini="/media/fat/MiSTer.ini.magik"
    ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    echo "ERROR: expected -zaparoo or -magik" >&2
    usage >&2
    exit 2
    ;;
esac

echo "==> Replacing MiSTer.ini with the saved $label configuration"
"$MISTER" run "
set -e
ZAPAROO=/media/fat/MiSTer.ini.zaparoo
MAGIK=/media/fat/MiSTer.ini.magik
TARGET=/media/fat/MiSTer.ini
TMP=/media/fat/.MiSTer.ini.frontend-switch
trap 'rm -f \"\$TMP\"' EXIT

test -s \"\$ZAPAROO\" || {
  echo \"ERROR: \$ZAPAROO is missing or empty\"
  exit 1
}
test -s \"\$MAGIK\" || {
  echo \"ERROR: \$MAGIK is missing or empty\"
  exit 1
}

cp '$source_ini' \"\$TMP\"
sync
mv \"\$TMP\" \"\$TARGET\"
sync
trap - EXIT
"

echo "==> Rebooting into $label"
"$MISTER" reboot-wait --raw

echo "Done. $label is selected."
