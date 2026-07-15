#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Fixed host-side MiSTer MagiK layouts. Developer commands default to dev;
# release/public commands must explicitly select public.
magik_layout_select() {
  case "${1:-dev}" in
    dev)
      MISTER_MAGIK_LAYOUT=dev
      MISTER_MAGIK_APP_DIR=/media/fat/mister-magik-dev
      MISTER_MAGIK_MAIN=/media/fat/MiSTer_MagiKDev
      MISTER_MAGIK_MAIN_NAME=MiSTer_MagiKDev
      ;;
    public)
      MISTER_MAGIK_LAYOUT=public
      MISTER_MAGIK_APP_DIR=/media/fat/mister-magik
      MISTER_MAGIK_MAIN=/media/fat/MiSTer_MagiK
      MISTER_MAGIK_MAIN_NAME=MiSTer_MagiK
      ;;
    *)
      echo "ERROR: unknown MiSTer MagiK layout: $1" >&2
      return 2
      ;;
  esac
  MISTER_MAGIK_BIN="$MISTER_MAGIK_APP_DIR/mister-magik-fb"
  MISTER_MAGIK_CATALOG_BUILDER="$MISTER_MAGIK_APP_DIR/mister-magik-catalog-builder"
  MISTER_MAGIK_MANIFEST="$MISTER_MAGIK_APP_DIR/platform-v1.manifest"
  MISTER_MAGIK_LAUNCHER_ENV="$MISTER_MAGIK_APP_DIR/launcher.env"
  MISTER_MAGIK_LIBRARY_DB="$MISTER_MAGIK_APP_DIR/library.sqlite3"
  MISTER_MAGIK_ASSET_DIR="$MISTER_MAGIK_APP_DIR/assets"
  export MISTER_MAGIK_LAYOUT MISTER_MAGIK_APP_DIR MISTER_MAGIK_MAIN MISTER_MAGIK_MAIN_NAME
  export MISTER_MAGIK_BIN MISTER_MAGIK_CATALOG_BUILDER MISTER_MAGIK_MANIFEST
  export MISTER_MAGIK_LAUNCHER_ENV MISTER_MAGIK_LIBRARY_DB MISTER_MAGIK_ASSET_DIR
}

magik_layout_select "${MISTER_MAGIK_LAYOUT:-dev}"
