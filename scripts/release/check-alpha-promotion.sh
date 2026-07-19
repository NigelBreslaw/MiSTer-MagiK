#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

channel="${1:?release channel is required}"
alpha_sha="${2:-}"
candidate_sha="${3:?candidate SHA is required}"

if [ "$channel" != beta ]; then
  exit 0
fi
if [ -z "$alpha_sha" ]; then
  echo "Beta publication requires an existing alpha tag." >&2
  exit 1
fi
if [ "$alpha_sha" != "$candidate_sha" ]; then
  echo "Beta publication requires the tested alpha commit $alpha_sha; got $candidate_sha." >&2
  exit 1
fi
