"""Archive incident evidence without bootstrap, repair, restart, or activation."""

from __future__ import annotations
import hashlib
import json
import subprocess
import time
from pathlib import Path
from .capture import capture_png


def capture(
    agent,
    run: Path,
    *,
    framebuffer=False,
    usb_seconds=None,
    runner=subprocess.run,
    agent_status=None,
):
    folder = run / "fpga-incident"
    folder.mkdir()  # Never overwrite a previous capture.
    if agent_status is not None:
        (folder / "agent-status.json").write_text(
            json.dumps(dict(agent_status.fields), indent=2) + "\n"
        )
    timeline = []

    def stage(name, action):
        event = {
            "stage": name,
            "started_monotonic_ns": time.monotonic_ns(),
            "started_unix_ns": time.time_ns(),
        }
        try:
            action()
            event["status"] = "saved"
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            event.update(status="failed", error=str(error))
        event["ended_monotonic_ns"] = time.monotonic_ns()
        timeline.append(event)
        (folder / "timeline.json").write_text(json.dumps(timeline, indent=2) + "\n")

    def diagnostics(name):
        value = agent.device_operation("fpga-evidence")
        (folder / name).write_text(json.dumps(value, indent=2) + "\n")

    stage("diagnostics-before", lambda: diagnostics("fpga-before.json"))
    if framebuffer:

        def frame():
            fields, pixels = agent.capture_framebuffer()
            # Preserve raw authoritative evidence even if conversion rejects it.
            (folder / "framebuffer.json").write_text(
                json.dumps(dict(fields), indent=2) + "\n"
            )
            (folder / "framebuffer.rgb565").write_bytes(pixels)
            png, _ = capture_png(fields, pixels, "raw")
            (folder / "framebuffer.png").write_bytes(png)

        stage("framebuffer", frame)
    if usb_seconds is not None:
        root = Path(__file__).resolve().parents[3]

        def usb():
            with (folder / "usb-capture.log").open("w") as log:
                runner(
                    [
                        str(root / "scripts/magik-ci"),
                        "capture-usb",
                        "--output",
                        str((folder / "usb-video.mov").resolve()),
                        "--seconds",
                        str(usb_seconds),
                    ],
                    cwd=root,
                    check=True,
                    timeout=180,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )

        stage("usb-video", usb)
    stage("diagnostics-after", lambda: diagnostics("fpga-after.json"))
    files = sorted(p for p in folder.iterdir() if p.is_file())
    (folder / "SHA256SUMS").write_text(
        "".join(
            hashlib.sha256(p.read_bytes()).hexdigest() + "  " + p.name + "\n"
            for p in files
        )
    )
    print(folder)
    return 0 if all(e["status"] == "saved" for e in timeline) else 1


def first_source(report):
    """Fingerprint source evidence only; later output snapshots may differ."""
    records = [s.get("decoded", {}) for s in report.get("samples", [])]
    records = [s for s in records if s.get("schema") == 24 and s.get("first_selected")]
    if not records or any(not s.get("crc_valid") for s in records):
        return None
    sources = [
        {
            key: s.get(key)
            for key in (
                "record_valid",
                "cause",
                "ledger_valid",
                "physical_depth",
                "physical_phase",
                "production_depth",
                "production_phase",
            )
        }
        for s in records
    ]
    if any(s != sources[0] for s in sources):
        return None
    return sources[0]


def collect(agent, status, run, arguments):
    if arguments.poll_count == 1:
        return capture(
            agent,
            run,
            framebuffer=arguments.framebuffer,
            usb_seconds=arguments.usb_seconds,
            agent_status=status,
        )
    index = []
    previous = None
    seen = set()
    result = 0
    try:
        for number in range(arguments.poll_count):
            folder = run / f"poll-{number:04d}"
            folder.mkdir()
            started = time.monotonic_ns()
            result = max(
                result,
                capture(
                    agent,
                    folder,
                    framebuffer=arguments.framebuffer,
                    usb_seconds=arguments.usb_seconds,
                    agent_status=status,
                ),
            )
            path = folder / "fpga-incident" / "fpga-before.json"
            report = json.loads(path.read_text()) if path.exists() else {}
            source = first_source(report)
            before = report.get("before", {}).get("boot_id")
            after = report.get("after", {}).get("boot_id")
            boot = before if before and before == after else None
            event = {
                "sample": number,
                "started_monotonic_ns": started,
                "ended_monotonic_ns": time.monotonic_ns(),
                "source": source,
                "boot_id": boot,
                "path": str(path.relative_to(run)),
            }
            if source and source["record_valid"] and boot:
                fingerprint = hashlib.sha256(
                    json.dumps([boot, source], sort_keys=True).encode()
                ).hexdigest()
                event["source_fingerprint"] = fingerprint
                if fingerprint not in seen:
                    lower = (
                        previous["started_monotonic_ns"]
                        if previous
                        and previous["boot_id"] == boot
                        and previous["source"]
                        and not previous["source"]["record_valid"]
                        else None
                    )
                    event["first_seen_interval_monotonic_ns"] = [
                        lower,
                        event["ended_monotonic_ns"],
                    ]
                    seen.add(fingerprint)
            index.append(event)
            previous = event
            (run / "poll-index.json").write_text(json.dumps(index, indent=2) + "\n")
            if number + 1 < arguments.poll_count:
                time.sleep(
                    max(
                        0,
                        arguments.poll_interval - (time.monotonic_ns() - started) / 1e9,
                    )
                )
    finally:
        if (run / "poll-index.json").exists():
            (run / "poll-index.sha256").write_text(
                hashlib.sha256((run / "poll-index.json").read_bytes()).hexdigest()
                + "  poll-index.json\n"
            )
    return result
