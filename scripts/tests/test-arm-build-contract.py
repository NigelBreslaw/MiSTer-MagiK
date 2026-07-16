#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contract for reproducible ARM build wrappers and check mode."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
apple = (ROOT / "magik-gui/build-arm64-apple-container.sh").read_text()
cross = (ROOT / "magik-gui/build-arm.sh").read_text()
licenses = (ROOT / "magik-gui/src/licenses.rs").read_text()

for name, text in (("Apple", apple), ("cross", cross)):
    assert "--check" in text, f"{name} wrapper has no check mode"
    assert "--lib-only" in text, f"{name} wrapper has no library-only mode"
    assert "MISTER_MAGIK_BUILD_TIME" in text
    assert "git -C" in text and "--date='format:%-d.%-m.%Y %H:%M'" in text
    assert "date +" not in text, f"{name} wrapper uses volatile wall-clock metadata"
    assert "STAGED_LICENSE" not in text, f"{name} wrapper mutates package inputs"

assert 'include_str!("../../LICENSE")' in licenses
assert 'include_str!("../LICENSE")' not in licenses

check_exit = apple.index('if [ "$COMMAND" = check ]')
for forbidden in ("MIRROR_BIN=", "bench_context_write_build_receipt", "record-binary-size.sh"):
    assert check_exit < apple.index(forbidden), f"check mode reaches {forbidden}"

print("ARM build contract ok")
