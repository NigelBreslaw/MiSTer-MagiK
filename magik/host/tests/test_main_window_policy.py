"""Compile and execute the provider's portable mapping policy on the host."""

from pathlib import Path
import shutil
import subprocess

import pytest


def test_main_window_mapping_policy(tmp_path):
    compiler = shutil.which("cc")
    if compiler is None:
        pytest.skip("host C compiler unavailable")
    source = (
        Path(__file__).resolve().parents[3]
        / "mister/platform/kernel/main-window/mister_magik_main_window_policy_test.c"
    )
    binary = tmp_path / "main-window-policy"
    subprocess.run(
        [
            compiler,
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            str(source),
            "-o",
            str(binary),
        ],
        check=True,
    )
    subprocess.run([str(binary)], check=True)
