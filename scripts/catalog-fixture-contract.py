#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Build and validate corpus-keyed catalog benchmark facts."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import tempfile
from typing import Any


class ContractError(ValueError):
    pass


def read_object(path: pathlib.Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ContractError(f"malformed {label}: {exc}") from exc
    if not isinstance(value, dict):
        raise ContractError(f"malformed {label}: root must be an object")
    return value


def integer(value: Any, label: str, *, minimum: int = 0) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise ContractError(f"malformed {label}: expected integer >= {minimum}")
    return value


def string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ContractError(f"malformed {label}: expected non-empty string")
    return value


def fixture_for(contract: dict[str, Any], facts: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    if integer(contract.get("schema"), "contract schema", minimum=1) != 1:
        raise ContractError("unsupported contract schema")
    fixtures = contract.get("fixtures")
    if not isinstance(fixtures, dict) or not fixtures:
        raise ContractError("malformed contract fixtures")
    fingerprint = string(facts.get("catalog_stamp_fingerprint"), "catalog fingerprint")
    fixture = fixtures.get(fingerprint)
    if fixture is None:
        raise ContractError(f"unknown catalog fixture fingerprint: {fingerprint}")
    if not isinstance(fixture, dict):
        raise ContractError(f"malformed fixture for fingerprint: {fingerprint}")
    return fingerprint, fixture


def validate(
    contract: dict[str, Any], facts: dict[str, Any], *, enforce_budgets: bool
) -> tuple[str, str, list[str]]:
    if integer(facts.get("schema"), "facts schema", minimum=1) != 1:
        raise ContractError("unsupported facts schema")
    fingerprint, fixture = fixture_for(contract, facts)
    fixture_id = string(fixture.get("id"), "fixture id")
    for field in ("summary_schema", "catalog_schema_version", "catalog_build_version"):
        expected = integer(fixture.get(field), f"fixture {field}")
        actual = integer(facts.get(field), f"facts {field}")
        if actual != expected:
            raise ContractError(f"{field} mismatch: expected={expected} actual={actual}")

    expected_counts = fixture.get("counts")
    actual_counts = facts.get("counts")
    if not isinstance(expected_counts, dict) or not isinstance(actual_counts, dict):
        raise ContractError("malformed counts")
    required_counts = ("games", "game_rows", "launcher_visible", "launcher_rows", "systems")
    for field in required_counts:
        expected = integer(expected_counts.get(field), f"fixture count {field}")
        actual = integer(actual_counts.get(field), f"facts count {field}")
        if actual != expected:
            raise ContractError(f"count {field} mismatch: expected={expected} actual={actual}")
    if integer(actual_counts["games"], "games") != integer(actual_counts["game_rows"], "game_rows"):
        raise ContractError("relational parity failed: games != game_rows")
    if integer(actual_counts["launcher_visible"], "launcher_visible") != integer(
        actual_counts["launcher_rows"], "launcher_rows"
    ):
        raise ContractError("projection parity failed: launcher_visible != launcher_rows")
    if integer(facts.get("discoveries"), "discoveries", minimum=1) < 1:
        raise ContractError("relational invariant failed: discoveries must be positive")

    expected_anchors = fixture.get("anchors")
    actual_anchors = facts.get("anchors")
    if not isinstance(expected_anchors, dict) or not isinstance(actual_anchors, dict):
        raise ContractError("malformed anchors")
    for field, expected in expected_anchors.items():
        if isinstance(expected, bool) or not isinstance(expected, (int, str)):
            raise ContractError(f"malformed fixture anchor {field}")
        actual = actual_anchors.get(field)
        if actual != expected:
            raise ContractError(f"anchor {field} mismatch: expected={expected} actual={actual}")

    budgets = fixture.get("performance_budgets")
    metrics = facts.get("performance_metrics")
    if not isinstance(budgets, dict) or not isinstance(metrics, dict):
        raise ContractError("malformed performance budget or metrics")
    reports: list[str] = []
    budget_failures: list[str] = []
    for field, raw_limit in budgets.items():
        limit = integer(raw_limit, f"performance budget {field}")
        raw_value = metrics.get(field)
        if raw_value is None:
            reports.append(f"{field}=missing/{limit}")
            if enforce_budgets:
                budget_failures.append(f"{field}=missing/{limit}")
            continue
        value = integer(raw_value, f"performance metric {field}")
        reports.append(f"{field}={value}/{limit}")
        if value > limit:
            budget_failures.append(f"{field}={value}/{limit}")
    if enforce_budgets and budget_failures:
        raise ContractError("performance budget failed: " + ", ".join(budget_failures))
    return fingerprint, fixture_id, reports


def parse_anchor(value: str) -> tuple[str, int | str]:
    key, separator, raw = value.partition("=")
    if not separator or not key or not raw:
        raise argparse.ArgumentTypeError("anchor must be NAME=VALUE")
    try:
        return key, int(raw)
    except ValueError:
        return key, raw


def write_facts(args: argparse.Namespace) -> None:
    summary = read_object(args.summary, "catalog summary")
    anchors = dict(args.anchor)
    facts = {
        "schema": 1,
        "catalog_stamp_fingerprint": summary.get("catalog_stamp_fingerprint"),
        "summary_schema": summary.get("schema"),
        "catalog_schema_version": summary.get("catalog_schema_version"),
        "catalog_build_version": summary.get("catalog_build_version"),
        "counts": {
            "games": args.games,
            "game_rows": args.game_rows,
            "launcher_visible": summary.get("total_game_count"),
            "launcher_rows": args.launcher_rows,
            "systems": args.systems,
        },
        "discoveries": args.discoveries,
        "anchors": anchors,
        "performance_metrics": {
            "library_ready_ms": args.library_ready_ms,
            "library_db_saved_ms": args.library_db_saved_ms,
            "db_bytes": args.db_bytes,
        },
    }
    args.output.write_text(json.dumps(facts, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def validate_command(args: argparse.Namespace) -> None:
    contract = read_object(args.contract, "catalog fixture contract")
    facts = read_object(args.facts, "catalog facts")
    fingerprint, fixture_id, reports = validate(
        contract, facts, enforce_budgets=args.enforce_performance_budgets
    )
    budget_status = "enforced" if args.enforce_performance_budgets else "recorded-only"
    print(
        "catalog_fixture_contract_tsv"
        f"\tcontract_schema=1\tfixture={fixture_id}\tfingerprint={fingerprint}"
        f"\tcorrectness=pass\tperformance={budget_status}\tmetrics={','.join(reports)}"
    )


def self_test() -> None:
    fixture = {
        "schema": 1,
        "fixtures": {
            "abc": {
                "id": "test",
                "summary_schema": 3,
                "catalog_schema_version": 65,
                "catalog_build_version": 10,
                "counts": {
                    "games": 5,
                    "game_rows": 5,
                    "launcher_visible": 4,
                    "launcher_rows": 4,
                    "systems": 2,
                },
                "anchors": {"arcade_visible": 1, "sms_kind": "console"},
                "performance_budgets": {"library_ready_ms": 10, "db_bytes": 100},
            }
        },
    }
    facts = {
        "schema": 1,
        "catalog_stamp_fingerprint": "abc",
        "summary_schema": 3,
        "catalog_schema_version": 65,
        "catalog_build_version": 10,
        "counts": {
            "games": 5,
            "game_rows": 5,
            "launcher_visible": 4,
            "launcher_rows": 4,
            "systems": 2,
        },
        "discoveries": 6,
        "anchors": {"arcade_visible": 1, "sms_kind": "console"},
        "performance_metrics": {"library_ready_ms": 11, "db_bytes": 90},
    }
    validate(fixture, facts, enforce_budgets=False)
    cases = []
    for name, mutate in (
        ("unknown", lambda row: row.update(catalog_stamp_fingerprint="unknown")),
        ("count", lambda row: row["counts"].update(game_rows=6)),
        ("schema", lambda row: row.update(summary_schema=4)),
        ("malformed", lambda row: row.update(counts="bad")),
    ):
        candidate = json.loads(json.dumps(facts))
        mutate(candidate)
        try:
            validate(fixture, candidate, enforce_budgets=False)
        except ContractError:
            cases.append(name)
        else:
            raise AssertionError(f"self-test accepted {name}")
    try:
        validate(fixture, facts, enforce_budgets=True)
    except ContractError as exc:
        if "performance budget failed" not in str(exc):
            raise
        cases.append("budget")
    else:
        raise AssertionError("self-test accepted an exceeded enforced budget")
    malformed_path = pathlib.Path(tempfile.mkdtemp()) / "malformed.json"
    malformed_path.write_text("{", encoding="utf-8")
    try:
        read_object(malformed_path, "self-test")
    except ContractError:
        cases.append("malformed-json")
    else:
        raise AssertionError("self-test accepted malformed JSON")
    expected = {"unknown", "count", "schema", "malformed", "budget", "malformed-json"}
    if set(cases) != expected:
        raise AssertionError(f"self-test coverage mismatch: {cases}")
    print("catalog-fixture-contract self-test ok")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    commands = result.add_subparsers(dest="command", required=True)
    write = commands.add_parser("write-facts")
    write.add_argument("--summary", type=pathlib.Path, required=True)
    write.add_argument("--output", type=pathlib.Path, required=True)
    for name in ("games", "game-rows", "launcher-rows", "systems", "discoveries", "db-bytes"):
        write.add_argument(f"--{name}", type=int, required=True)
    write.add_argument("--library-ready-ms", type=int)
    write.add_argument("--library-db-saved-ms", type=int)
    write.add_argument("--anchor", action="append", default=[], type=parse_anchor)
    write.set_defaults(function=write_facts)
    check = commands.add_parser("validate")
    check.add_argument("--contract", type=pathlib.Path, required=True)
    check.add_argument("--facts", type=pathlib.Path, required=True)
    check.add_argument("--enforce-performance-budgets", action="store_true")
    check.set_defaults(function=validate_command)
    test = commands.add_parser("self-test")
    test.set_defaults(function=lambda _args: self_test())
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        args.function(args)
    except (ContractError, OSError) as exc:
        print(f"catalog fixture contract failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
