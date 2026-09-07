"""Retained device controls over the common native connection."""

import json

DISPLAY_MODES = (
    "auto",
    "hdmi-1280x720p60",
    "hdmi-1366x768p60",
    "hdmi-1920x1080p60",
    "hdmi-1920x1200p60",
    "hdmi-2048x1536p60",
    "hdmi-2560x1440p60",
    "crt-240p60",
    "crt-288p50",
    "crt-480p60",
    "crt-576p50",
)


def add_commands(commands):
    mode = commands.add_parser("mode").add_subparsers(dest="action", required=True)
    mode.add_parser("status")
    change = mode.add_parser("set")
    change.add_argument("mode", choices=("dev", "public", "stock"))
    change.add_argument("--attended", action="store_true", required=True)
    media = commands.add_parser("media").add_subparsers(dest="action", required=True)
    for name in ("check", "download"):
        item = media.add_parser(name)
        item.add_argument("--system", required=True)
        item.add_argument("--layout", choices=("dev", "public"), default="dev")
    catalog = commands.add_parser("catalog").add_subparsers(
        dest="action", required=True
    )
    for name in (
        "inspect",
        "metadata-qualification",
        "rom-audit",
        "neogeo-family-audit",
        "screenshots",
        "query",
        "cores",
        "purge",
        "publish",
        "screenshot-qualification",
    ):
        item = catalog.add_parser(name)
        item.add_argument("--layout", choices=("dev", "public"), default="dev")
        if name == "publish":
            from pathlib import Path

            item.add_argument("--release-dir", type=Path, required=True)
        if name == "purge":
            item.add_argument("--confirm", action="store_true", required=True)
        if name in {"screenshots", "screenshot-qualification"}:
            item.add_argument("--system", required=True)
        if name == "query":
            item.add_argument("--database", required=True)
            item.add_argument("--sql", required=True)
    recover = commands.add_parser("recover")
    recover.add_argument("--attended", action="store_true", required=True)
    reboot = commands.add_parser("reboot")
    reboot.add_argument("--attended", action="store_true", required=True)
    commands.add_parser("status")
    commands.add_parser("diagnostics")
    commands.add_parser("logs")
    launcher = commands.add_parser("launcher").add_subparsers(
        dest="action", required=True
    )
    for name in ("status", "restart", "return-to-launcher"):
        launcher.add_parser(name)
    display = commands.add_parser("display").add_subparsers(
        dest="action", required=True
    )
    display.add_parser("status")
    change = display.add_parser("set")
    change.add_argument("mode", choices=DISPLAY_MODES)
    change.add_argument("--attended", action="store_true", required=True)
    change.add_argument("--acknowledge-31khz", action="store_true")


def run_device(arguments, run):
    from .cli import connect_agent
    from .results import append_event

    group = arguments.device_command
    if group == "reboot":
        return reboot_device(arguments, run)
    if group == "catalog":
        return run_catalog(arguments, run)
    fields = {}
    if group == "recover":
        operation = "device-recover"
        fields = {"attended": arguments.attended}
    elif group == "media":
        operation = "media-operation"
        fields = {
            "action": arguments.action,
            "layout": arguments.layout,
            "system": arguments.system,
        }
    elif group in {"status", "launcher"}:
        action = getattr(arguments, "action", "status")
        operation = {
            "status": "device-status",
            "restart": "launcher-restart",
            "return-to-launcher": "launcher-return",
        }[action]
    elif group == "mode":
        operation = "mode-" + arguments.action
        if arguments.action == "set":
            fields = {"mode": arguments.mode, "attended": arguments.attended}
    elif group == "display":
        operation = "display-" + arguments.action
        if arguments.action == "set":
            fields = {"mode": arguments.mode, "attended": arguments.attended}
            if arguments.acknowledge_31khz:
                fields["acknowledge_31khz"] = True
    else:
        operation = "device-evidence"
    agent, _ = connect_agent(run, {"device-control-v1"})
    report = agent.device_operation(operation, fields)
    path = run / "device-operation.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    append_event(run, {"phase": operation, "outcome": "passed", "report": path.name})
    print(json.dumps(report, indent=2))
    return 0


def run_catalog(arguments, run):
    if arguments.action == "screenshot-qualification":
        from .catalog import qualify_screenshots

        return qualify_screenshots(arguments, run)
    if arguments.action == "publish":
        from .publication import databases

        return databases(arguments, run)
    from .cli import connect_agent
    from .client import AgentError
    from .results import append_event

    fields = {"action": arguments.action, "layout": arguments.layout}
    for name in ("system", "database", "sql", "confirm"):
        value = getattr(arguments, name, None)
        if value is not None:
            fields[name] = value
    agent, _ = connect_agent(run, {"catalog-operations-v1"})
    response, body = agent._request(
        "catalog-operation", fields, attempts=1, timeout=130
    )
    (run / "catalog-output.txt").write_bytes(body)
    (run / "catalog-result.json").write_text(
        json.dumps(dict(response.fields), indent=2) + "\n"
    )
    if response.operation == "error":
        raise AgentError.from_fields(response.fields)
    if (
        response.operation != "catalog-result"
        or response.fields.get("exit_code") != 0
        or response.fields.get("error")
    ):
        raise AgentError(f"catalog operation failed; see {run / 'catalog-result.json'}")
    if arguments.action == "metadata-qualification":
        from .catalog import validate_metadata

        validate_metadata(response.fields, body)
    append_event(
        run,
        {
            "phase": "catalog",
            "outcome": "passed",
            "action": arguments.action,
            "layout": arguments.layout,
        },
    )
    print(body.decode(errors="replace"))
    return 0


def reboot_device(arguments, run, *, agent=None):
    import time
    from .cli import connect_agent
    from .client import AgentError

    if agent is None:
        agent, _ = connect_agent(run, {"device-control-v1"})
    before = agent.device_operation("device-evidence")
    boot_id = before.get("boot_id", {}).get("text")
    if not boot_id:
        raise AgentError("cannot identify current boot; reboot not sent")
    report = {"before": before}
    try:
        # Never replay this mutation if its acknowledgement is lost.
        try:
            report["request"] = agent.device_operation(
                "device-reboot", {"attended": arguments.attended}
            )
        except (OSError, TimeoutError) as error:
            report["ambiguous_request"] = str(error)
        deadline = time.monotonic() + 90
        rediscovery_at = time.monotonic() + 30
        rediscovered = False
        while time.monotonic() < deadline:
            time.sleep(2)
            try:
                evidence = agent.device_operation(
                    "device-evidence",
                    timeout=min(5, max(0.1, deadline - time.monotonic())),
                )
                report["after"] = evidence
                if (
                    evidence.get("boot_id", {}).get("text") not in (None, boot_id)
                    and evidence.get("main_status", {}).get("launcher_ready_phase")
                    == "ready"
                ):
                    print("MiSTer rebooted and Main is ready.")
                    return 0
            except AgentError:
                # Authentication failures require changed access, not probing/retries.
                raise
            except (OSError, TimeoutError) as error:
                report["last_error"] = str(error)
                if not rediscovered and time.monotonic() >= rediscovery_at:
                    from .discovery import resolve_device, DiscoveryError
                    from .results import record_device

                    rediscovered = True
                    try:
                        resolved = resolve_device()
                        agent.host = resolved.address
                        record_device(run, resolved.identity, resolved.address)
                    except DiscoveryError as discovery_error:
                        report["discovery_error"] = str(discovery_error)
        raise AgentError(
            "reboot health not confirmed within 90 seconds; do not replay; use SD-card recovery if unstable"
        )
    finally:
        (run / "reboot.json").write_text(json.dumps(report, indent=2) + "\n")
