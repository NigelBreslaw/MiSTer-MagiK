#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for the bootstrap-free PixelText8 contract."""

from __future__ import annotations

import importlib.util
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CHECKER = ROOT / "scripts/checks/check-pixel-text-contract.py"

SPEC = importlib.util.spec_from_file_location("pixel_text_contract", CHECKER)
assert SPEC and SPEC.loader
CONTRACT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CONTRACT
SPEC.loader.exec_module(CONTRACT)

PRIMITIVE_TEXT = """
export enum PixelTextSize {
    px8,
    px16,
    px24,
    px32,
}
export component PixelText8 {
    Text { font-size: 8px; }
}
"""

JERSEY_PRIMITIVE_TEXT = """
export component JerseyTitleText inherits Text {
    font-size: 41px;
}
"""


def primitives() -> list[object]:
    return [
        CONTRACT.Source(CONTRACT.PRIMITIVE, PRIMITIVE_TEXT),
        CONTRACT.Source(CONTRACT.JERSEY_PRIMITIVE, JERSEY_PRIMITIVE_TEXT),
    ]


class ContractTests(unittest.TestCase):
    def test_all_legal_enum_values_are_required_in_order(self) -> None:
        self.assertEqual(
            CONTRACT.EXPECTED_ENUM_VALUES,
            ("px8", "px16", "px24", "px32"),
        )
        CONTRACT.check_sources(primitives())
        for invalid in (
            PRIMITIVE_TEXT.replace("    px8,\n", ""),
            PRIMITIVE_TEXT.replace("    px16,\n", "    px12,\n"),
            PRIMITIVE_TEXT.replace("    px32,\n", "    px32,\n    px40,\n"),
        ):
            with self.assertRaises(CONTRACT.ContractError):
                CONTRACT.check_sources(
                    [
                        CONTRACT.Source(CONTRACT.PRIMITIVE, invalid),
                        CONTRACT.Source(CONTRACT.JERSEY_PRIMITIVE, JERSEY_PRIMITIVE_TEXT),
                    ]
                )

    def test_raw_text_and_direct_font_size_fixtures_are_rejected(self) -> None:
        for body, message in (
            ("Text { text: \"bad\"; }", "raw Text is forbidden"),
            ("PixelText8 { font-size: 12px; }", "direct font-size is forbidden"),
        ):
            with self.subTest(body=body):
                with self.assertRaisesRegex(CONTRACT.ContractError, message):
                    CONTRACT.check_sources(
                        [
                            *primitives(),
                            CONTRACT.Source(
                                PurePosixPath("apps/mister/ui/fixture.slint"),
                                body,
                            ),
                        ]
                    )

    def test_comments_and_strings_do_not_trigger_the_contract(self) -> None:
        CONTRACT.check_sources(
            [
                *primitives(),
                CONTRACT.Source(
                    PurePosixPath("apps/mister/ui/fixture.slint"),
                    '// Text { font-size: 12px; }\n'
                    'property <string> example: "Text { font-size: 12px; }";\n'
                    "PixelText8 { size: PixelTextSize.px8; }\n",
                ),
            ]
        )

    def test_staged_scope_reads_index_instead_of_working_tree(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pixel-text-contract-") as name:
            repository = Path(name)
            subprocess.run(["git", "init", "-q"], cwd=repository, check=True)
            primitive = repository / CONTRACT.PRIMITIVE
            primitive.parent.mkdir(parents=True)
            primitive.write_text(PRIMITIVE_TEXT)
            jersey_primitive = repository / CONTRACT.JERSEY_PRIMITIVE
            jersey_primitive.write_text(JERSEY_PRIMITIVE_TEXT)
            scene = repository / "apps/mister/ui/scene.slint"
            scene.write_text("PixelText8 { size: PixelTextSize.px8; }\n")
            subprocess.run(
                ["git", "add", "--", str(CONTRACT.UI_ROOT)],
                cwd=repository,
                check=True,
            )
            scene.write_text("Text { font-size: 12px; }\n")

            staged = subprocess.run(
                [
                    sys.executable,
                    str(CHECKER),
                    "--repository",
                    str(repository),
                    "--staged",
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(staged.returncode, 0, staged.stderr)

            full = subprocess.run(
                [
                    sys.executable,
                    str(CHECKER),
                    "--repository",
                    str(repository),
                    "--all",
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(full.returncode, 1)
            self.assertIn("raw Text is forbidden", full.stderr)


if __name__ == "__main__":
    unittest.main()
