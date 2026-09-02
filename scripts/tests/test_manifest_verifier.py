# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import contextlib
import io
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import manifest
from scripts.magik_ci.cli import parser


class ManifestVerifierTests(unittest.TestCase):
    def test_layout_is_required_and_known(self):
        with self.assertRaises(TypeError):
            manifest.verify(Path("unused"))
        with self.assertRaisesRegex(ValueError, "invalid_platform_layout"):
            manifest.verify(Path("unused"), layout="unknown")
        for args in [[], ["--layout", "unknown"]]:
            with (
                contextlib.redirect_stderr(io.StringIO()),
                self.assertRaises(SystemExit),
            ):
                parser().parse_args(
                    ["ci", "platform-manifest", "verify", "unused", *args]
                )

    def test_missing_verifier_never_falls_back(self):
        with (
            patch.dict(
                "os.environ", {"MISTER_MAGIK_MANIFEST_CHECK": "/does-not-exist/magik"}
            ),
            self.assertRaisesRegex(FileNotFoundError, "verifier missing"),
        ):
            manifest.verify(Path("unused"), layout="public")

    def test_contract_rejection_propagates(self):
        with (
            patch.object(Path, "is_file", return_value=True),
            patch.object(
                manifest.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(
                    [], 1, "", "platform_path_mismatch: manager"
                ),
            ) as run,
        ):
            with self.assertRaisesRegex(ValueError, "platform_path_mismatch: manager"):
                manifest.verify(Path("bad.manifest"), layout="public")
            self.assertIn("public", run.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
