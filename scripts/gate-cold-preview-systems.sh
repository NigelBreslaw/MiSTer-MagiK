#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Gate first-preview readiness for installed screenshot-pack systems.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$HERE/scripts/profile-cold-preview-systems.sh" "$@" --require-pass
