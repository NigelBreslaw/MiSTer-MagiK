#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Static contract for reproducible ARM build wrappers and check mode."""

from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
apple = (ROOT / "apps/mister/build-arm64-apple-container.sh").read_text()
cross = (ROOT / "apps/mister/build-arm.sh").read_text()
licenses = (ROOT / "apps/mister/src/licenses.rs").read_text()
ffmpeg = (ROOT / "apps/mister/scripts/build-minimal-ffmpeg.sh").read_text()

for name, text in (("Apple", apple), ("cross", cross)):
    assert "--check" in text, f"{name} wrapper has no check mode"
    assert "--lib-only" in text, f"{name} wrapper has no library-only mode"
    assert "MISTER_MAGIK_BUILD_TIME" in text
    assert "git -C" in text and "--date='format:%-d.%-m.%Y %H:%M'" in text
    assert "date +" not in text, f"{name} wrapper uses volatile wall-clock metadata"
    assert "STAGED_LICENSE" not in text, f"{name} wrapper mutates package inputs"
    assert '--fast) PROFILE=release' in text, f"{name} wrapper has no fast release profile"

for wrapper in ("apps/mister/build-arm.sh", "apps/mister/build-arm64-apple-container.sh"):
    help_text = subprocess.run(
        [str(ROOT / wrapper), "--help"],
        cwd=ROOT,
        check=True,
        text=True,
        capture_output=True,
    ).stdout
    assert "--fast" in help_text, f"{wrapper} help omits --fast"

deploy = (ROOT / "scripts/deploy-rust.sh").read_text()
bench = (ROOT / "scripts/bench-debug-build.sh").read_text()
assert '--fast) PROFILE=release; BUILD_FLAG=(--fast)' in deploy
assert 'build-ui-fast) echo "apps/mister/build-arm.sh --fast"' in bench
assert 'build-ui-fast) profile="release"' in bench
assert 'restore_stamp="$(mktemp ' in bench
assert 'touch -r "$restore_stamp" "$restore_path"' in bench
assert "image arch probe" not in apple
assert 'container run --arch arm64 --rm "$IMAGE" uname -m' in apple
assert "VERIFIED_STAMP=" in ffmpeg and "verified_cache_is_current" in ffmpeg
assert 'agent deploy-magik-bin "$BIN" "$REMOTE" --json' in deploy
assert "Reusing agent-verified transfer hash" in deploy

arcade_profile = (ROOT / "scripts/profile-arcade-scroll.sh").read_text()
assert '--fast) build_profile="release"' in arcade_profile
assert '"$HERE/scripts/deploy-rust.sh" --fast --ui-scope launcher --bench-tools' in arcade_profile

assert 'include_str!("../../../LICENSE")' in licenses
assert 'include_str!("../LICENSE")' not in licenses

check_exit = apple.index('if [ "$COMMAND" = check ]')
for forbidden in ("MIRROR_BIN=", "bench_context_write_build_receipt", "record-binary-size.sh"):
    assert check_exit < apple.index(forbidden), f"check mode reaches {forbidden}"

print("ARM build contract ok")
