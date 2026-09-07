"""Strict, host-only comparison of isolated fresh catalog reports."""

import re

DIGESTS = frozenset(
    f"{name}_sha256" for name in ("identity", "ordering", "launch", "search", "artifact_set")
)


def catalog_identity(report):
    assert report.get("ok") is True, "catalog build failed"
    systems = report["systems"]
    assert isinstance(systems, list) and systems, "missing systems"
    by_id = {}
    for system in systems:
        key = system["system_id"]
        assert isinstance(key, str) and key, "invalid system ID"
        assert key not in by_id, f"duplicate system: {key}"
        assert re.fullmatch(r"[0-9a-f]{64}", system["rows_sha256"]), "invalid row fingerprint"
        for field in ("games", "variants", "order"):
            assert type(system[field]) is int and system[field] >= 0, f"invalid {field}"
        for field in ("display_title", "section", "family"):
            assert isinstance(system[field], str), f"invalid {field}"
        by_id[key] = system
    assert sum(system["games"] for system in systems) > 0, "empty catalog"
    summaries = [
        line for line in report["inspection"].splitlines()
        if line.startswith("catalog_v3_summary_tsv\t")
    ]
    assert len(summaries) == 1, "missing or duplicate integrity summary"
    fields = {}
    for field in summaries[0].split("\t")[1:]:
        key, value = field.split("=", 1)
        assert key not in fields, f"duplicate summary field: {key}"
        fields[key] = value
    assert fields["valid"] == "1", "invalid catalog"
    assert int(fields["total_games"]) == sum(system["games"] for system in systems)
    hashes = {key: fields[key] for key in DIGESTS}
    assert all(re.fullmatch(r"[0-9a-f]{64}", value) for value in hashes.values()), "invalid digest"
    return by_id, hashes


def assert_catalog_equivalent(before, after):
    before_systems, before_hashes = catalog_identity(before)
    after_systems, after_hashes = catalog_identity(after)
    assert before_systems == after_systems, "catalog rows, variants or system metadata changed"
    assert before_hashes == after_hashes, "published catalog artifacts or behavior changed"


def restore_application(agent, run, retain_diagnostics):
    try:
        retain_diagnostics(run, agent)
    finally:
        agent.start(restart=True, expected_sha256=agent.expected_sha256)
