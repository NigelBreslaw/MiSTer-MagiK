#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Gate wrapper for cold direct-to-system turbo preview coverage.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$HERE/scripts/profile-cold-turbo-preview.sh" "$@" --require-pass
