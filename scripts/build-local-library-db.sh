#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

echo "ERROR: build-local-library-db was retired with Catalog V2; use catalog-lab and the V3 standalone builder" >&2
exit 2

usage() {
  cat >&2 <<'USAGE'
usage: scripts/build-local-library-db.sh --mirror DIR --out DB [--target /media/fat]

Build a MiSTer MagiK library.sqlite3 on the host from a local mirror of the
MiSTer SD card. The scanner walks DIR, but persisted launch paths are rewritten
to /media/fat paths by default.
USAGE
}

mirror=""
out=""
target="/media/fat"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mirror)
      mirror="${2:-}"
      shift 2
      ;;
    --out)
      out="${2:-}"
      shift 2
      ;;
    --target)
      target="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$mirror" || -z "$out" ]]; then
  usage
  exit 2
fi

mirror="${mirror%/}"
target="${target%/}"

if [[ ! -d "$mirror" ]]; then
  echo "mirror is not a directory: $mirror" >&2
  exit 1
fi

mkdir -p "$(dirname "$out")"

export MISTER_LIBRARY_ROOTS="$mirror/_Arcade|$mirror/games|$mirror/_DOS Games|$mirror/_LLAPI"
export MISTER_LIBRARY_PATH_MAP="$mirror=$target"
export MISTER_LIBRARY_BENCH_SQLITE="$out"
export MISTER_LIBRARY_SCAN_BENCH_FORCE=1
export MISTER_LIBRARY_SCAN_BENCH_ITERATIONS="${MISTER_LIBRARY_SCAN_BENCH_ITERATIONS:-1}"

cargo run --manifest-path crates/catalog/Cargo.toml --bin library-scan-bench
