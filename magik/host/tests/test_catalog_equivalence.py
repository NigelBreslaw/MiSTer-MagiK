import copy
import importlib.util
from pathlib import Path
from unittest.mock import Mock

import pytest

spec = importlib.util.spec_from_file_location(
    "catalog_equivalence",
    Path(__file__).resolve().parents[2] / "scenarios/catalog_equivalence.py",
)
comparison = importlib.util.module_from_spec(spec)
spec.loader.exec_module(comparison)


def report():
    return {
        "ok": True,
        "systems": [
            {
                "system_id": "arcade",
                "games": 1,
                "variants": 0,
                "order": 0,
                "display_title": "Arcade",
                "section": "Arcade",
                "family": "arcade",
                "rows_sha256": "a" * 64,
            }
        ],
        "inspection": "catalog_v3_summary_tsv\tvalid=1\ttotal_games=1\t"
        + "\t".join(f"{name}={'b' * 64}" for name in sorted(comparison.DIGESTS)),
    }


def test_operational_fields_do_not_change_identity():
    before = report()
    after = copy.deepcopy(before)
    before["artifact_sha256"] = "old"
    after.update(artifact_sha256="new", destination="/different", build={"elapsed": 5})
    comparison.assert_catalog_equivalent(before, after)


@pytest.mark.parametrize(
    "case",
    [
        "missing_digest",
        "duplicate_system",
        "bad_row_hash",
        "bad_digest",
        "failed",
        "rows",
        "ordering",
        "artifact",
        "missing_metadata",
        "duplicate_summary",
    ],
)
def test_invalid_or_changed_catalog_fails(case):
    before, after = report(), report()
    if case == "missing_digest":
        after["inspection"] = after["inspection"].replace(
            "\tlaunch_sha256=" + "b" * 64, ""
        )
    elif case == "duplicate_system":
        after["systems"] *= 2
    elif case == "bad_row_hash":
        after["systems"][0]["rows_sha256"] = "broken"
    elif case == "bad_digest":
        after["inspection"] = after["inspection"].replace(
            "search_sha256=" + "b" * 64, "search_sha256=no"
        )
    elif case == "failed":
        after["ok"] = False
    elif case == "rows":
        after["systems"][0]["rows_sha256"] = "c" * 64
    elif case == "ordering":
        after["systems"][0]["order"] = 1
    elif case == "artifact":
        after["inspection"] = after["inspection"].replace(
            "artifact_set_sha256=" + "b" * 64, "artifact_set_sha256=" + "c" * 64
        )
    elif case == "missing_metadata":
        del after["systems"][0]["family"]
    else:
        after["inspection"] += "\n" + after["inspection"]
    with pytest.raises((AssertionError, KeyError, ValueError)):
        comparison.assert_catalog_equivalent(before, after)


def test_restoration_runs_when_diagnostics_fail():
    agent = Mock(expected_sha256="sha")
    with pytest.raises(RuntimeError):
        comparison.restore_application(
            agent, Path("/unused"), Mock(side_effect=RuntimeError)
        )
    agent.start.assert_called_once_with(restart=True, expected_sha256="sha")
