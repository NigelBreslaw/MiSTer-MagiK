"""Explicit catalog qualification; never an application-delivery prerequisite."""

import json
import re
from .client import AgentError


def validate_metadata(fields, body):
    reports = []
    for line in body.decode().splitlines():
        try:
            reports.append(json.loads(line))
        except ValueError:
            continue
    if not reports or not isinstance(reports[-1], dict):
        raise AgentError("metadata report omitted JSON")
    report = reports[-1]
    compact = report.get("compact", {})
    if (
        report.get("schema") != "mister-magik-runtime-metadata-qualification-v2"
        or compact.get("valid") is not True
        or compact.get("shard_count") != 35
        or not 0 < compact.get("file_bytes", 0) <= 8 * 1024 * 1024
        or any(
            compact.get(name, 0) <= 0
            for name in (
                "software_rows",
                "arcade_mame_rows",
                "arcade_hbmame_rows",
                "arcade_mister_rows",
            )
        )
        or fields.get("legacy_sqlite_absence", {}).get("all_absent") is not True
    ):
        raise AgentError("metadata qualification failed; raw report retained")


def qualify_screenshots(arguments, run):
    from .cli import connect_agent

    agent, _ = connect_agent(run, {"catalog-operations-v1", "device-control-v1"})
    media = agent.device_operation(
        "media-operation",
        dict(action="qualify", layout=arguments.layout, system=arguments.system),
    )
    (run / "screenshot-media.json").write_text(json.dumps(media, indent=2) + "\n")

    def request(action, **extra):
        header, body = agent._request(
            "catalog-operation",
            dict(
                action=action, layout=arguments.layout, system=arguments.system, **extra
            ),
            attempts=1,
            timeout=130,
        )
        (run / (action + ".txt")).write_bytes(body)
        (run / (action + ".json")).write_text(
            json.dumps(dict(header.fields), indent=2) + "\n"
        )
        if (
            header.operation != "catalog-result"
            or header.fields.get("exit_code") != 0
            or header.fields.get("error")
        ):
            raise AgentError(
                "screenshot qualification operation failed; evidence retained"
            )
        return body.decode() + "\n" + header.fields.get("stderr", "")

    audit = request("screenshots")
    summary = next(
        (
            line
            for line in audit.splitlines()
            if line.startswith("catalog_screenshot_summary_tsv\t")
        ),
        "",
    )
    fields = dict(item.split("=", 1) for item in summary.split("\t")[1:] if "=" in item)
    if (
        fields.get("valid") != "1"
        or fields.get("system") != arguments.system
        or int(fields.get("games", 0)) <= 0
        or int(fields.get("available", 0)) <= 0
    ):
        raise AgentError("screenshot audit summary is incomplete; evidence retained")
    rows = [line.split("\t") for line in audit.splitlines()]
    selected = next(
        (row[2] for row in rows if len(row) >= 5 and row[4] == "1" and row[2]), None
    )
    if not selected:
        raise AgentError("screenshot qualification found no available asset")
    probe = request("preview-render", asset_key=selected)
    record = next(
        (
            line
            for line in probe.splitlines()
            if line.startswith("preview_render_probe_tsv\t")
        ),
        "",
    )
    fields = dict(item.split("=", 1) for item in record.split("\t")[1:] if "=" in item)
    if (
        fields.get("valid") != "1"
        or int(fields.get("rendered_pixels", 0)) <= 0
        or fields.get("load_source") != "index_pread"
        or not re.fullmatch(r"[0-9a-f]{64}", fields.get("pixel_sha256", ""))
    ):
        raise AgentError("screenshot render qualification failed; evidence retained")
    print(
        json.dumps(
            {"system": arguments.system, "selected_asset": selected, "render": fields},
            indent=2,
        )
    )
    return 0
