#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for the typed shared launcher contract ratchet."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[2]
CHECKER = ROOT / "scripts/checks/check-launcher-contract.py"

SPEC = importlib.util.spec_from_file_location("launcher_contract", CHECKER)
assert SPEC and SPEC.loader
CONTRACT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CONTRACT
SPEC.loader.exec_module(CONTRACT)


def source(text: str, suffix: str = ".slint") -> object:
    return CONTRACT.Source(
        PurePosixPath(f"apps/mister/ui/fixture{suffix}"),
        text,
    )


class ContractTests(unittest.TestCase):
    def test_typed_state_and_dynamic_numbers_are_allowed(self) -> None:
        CONTRACT.check_sources(
            [
                source(
                    """
                    enum Screen { home, settings }
                    global NavigationView {
                        in property <Screen> screen;
                        in property <int> selected-index;
                        in property <int> item-count;
                    }
                    if NavigationView.screen == Screen.home : Rectangle { }
                    if NavigationView.selected-index == 0 : Rectangle { }
                    """
                )
            ]
        )

    def test_retired_bridge_symbols_are_rejected(self) -> None:
        for symbol in CONTRACT.FORBIDDEN_SYMBOLS:
            with self.assertRaisesRegex(
                CONTRACT.ContractError, "retired launcher symbol"
            ):
                CONTRACT.check_sources(
                    [source(f"struct Fixture {{ {symbol}: usize }}", ".rs")]
                )

    def test_integer_finite_state_and_legacy_fields_are_rejected(self) -> None:
        for declaration in (
            "in property <int> screen-orientation;",
            "property <int> search-status;",
            "in property <bool> loading-visible;",
        ):
            with self.assertRaises(CONTRACT.ContractError):
                CONTRACT.check_sources([source(declaration)])

    def test_numeric_typed_state_comparisons_are_rejected(self) -> None:
        for comparison in (
            "if NavigationView.screen == 0 : Rectangle { }",
            "if 2 != root.transition-state : Rectangle { }",
        ):
            with self.assertRaisesRegex(CONTRACT.ContractError, "cannot be compared"):
                CONTRACT.check_sources([source(comparison)])

    def test_non_launcher_experiments_are_outside_the_semantic_guard(self) -> None:
        CONTRACT.check_sources(
            [
                CONTRACT.Source(
                    PurePosixPath("apps/mister/ui/bench/fixture.slint"),
                    "property <int> stripe-kind; if stripe-kind == 0 : Rectangle { }",
                )
            ]
        )


if __name__ == "__main__":
    unittest.main()
