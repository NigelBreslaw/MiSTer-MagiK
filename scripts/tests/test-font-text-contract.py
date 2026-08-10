#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for the bootstrap-free font-specific text contract."""

from __future__ import annotations

import importlib.util
from pathlib import Path, PurePosixPath
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
CHECKER = ROOT / "scripts/checks/check-font-text-contract.py"

SPEC = importlib.util.spec_from_file_location("font_text_contract", CHECKER)
assert SPEC and SPEC.loader
CONTRACT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CONTRACT
SPEC.loader.exec_module(CONTRACT)


def primitive_text(contract: object) -> str:
    values = "\n".join(f"    {value}," for value in contract.values)
    return f'''\
export enum {contract.enum_name} {{
{values}
}}
export component {contract.component} inherits Text {{
    in property <{contract.enum_name}> size: {contract.enum_name}.{contract.values[0]};
    font-family: "{contract.family}";
    font-size: {contract.renderer_size};
}}
'''


def primitives() -> list[object]:
    return [
        CONTRACT.Source(contract.path, primitive_text(contract))
        for contract in CONTRACT.CONTRACTS
    ]


class ContractTests(unittest.TestCase):
    def test_each_font_has_only_its_supported_sizes(self) -> None:
        CONTRACT.check_sources(primitives())
        for contract in CONTRACT.CONTRACTS:
            invalid = primitive_text(contract).replace(
                f"    {contract.values[-1]},", f"    {contract.values[-1]},\n    px99,"
            )
            sources = [
                CONTRACT.Source(item.path, invalid if item.path == contract.path else item.text)
                for item in primitives()
            ]
            with self.assertRaisesRegex(CONTRACT.ContractError, contract.enum_name):
                CONTRACT.check_sources(sources)

    def test_each_component_has_a_fixed_family_and_renderer_size(self) -> None:
        for contract in CONTRACT.CONTRACTS:
            for invalid, message in (
                (primitive_text(contract).replace(contract.family, "Wrong Family"), "family"),
                (primitive_text(contract).replace(contract.renderer_size, "99px"), "resolve"),
            ):
                sources = [
                    CONTRACT.Source(item.path, invalid if item.path == contract.path else item.text)
                    for item in primitives()
                ]
                with self.assertRaisesRegex(CONTRACT.ContractError, message):
                    CONTRACT.check_sources(sources)

    def test_raw_text_direct_size_and_legacy_api_are_rejected(self) -> None:
        for body, message in (
            ("Text { text: \"bad\"; }", "raw Text is forbidden"),
            ("Start2P { font-size: 12px; }", "direct font-size is forbidden"),
            ("PixelText8 { }", "legacy mixed-font text API is forbidden"),
        ):
            with self.assertRaisesRegex(CONTRACT.ContractError, message):
                CONTRACT.check_sources(
                    [
                        *primitives(),
                        CONTRACT.Source(PurePosixPath("apps/mister/ui/fixture.slint"), body),
                    ]
                )


if __name__ == "__main__":
    unittest.main()
