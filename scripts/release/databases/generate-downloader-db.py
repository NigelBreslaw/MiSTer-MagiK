#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Compatibility entrypoint; implementation lives in the typed CI surface."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from scripts.magik_ci.downloader_db import generate, main, validate_path

__all__ = ["generate", "main", "validate_path"]

if __name__ == "__main__":
    main()
