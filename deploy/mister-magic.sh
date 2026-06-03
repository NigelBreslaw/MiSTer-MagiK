#!/bin/bash
# MiSTer Scripts-menu entry for the Slint UI.
#
# Install location: /media/fat/Scripts/mister-magic.sh
# It appears in the MiSTer OSD under "Scripts" and simply hands off to the
# launcher that ships inside the bundle.
APP="/media/fat/mister-magic"

if [ ! -x "$APP/run-mister.sh" ]; then
    echo "mister-magic is not installed at $APP"
    echo "Build it with scripts/build-arm-bundle.sh and deploy it with"
    echo "scripts/deploy-mister.sh from the mister-magic project."
    exit 1
fi

exec "$APP/run-mister.sh"
