"""Cold fresh-catalog build: purge, one attended reboot, collect evidence, compare.

The reboot makes the first boot a real cold start: no catalog, an empty page
cache, and the launcher's intro running beside the fresh build. The Development
application records the build's CPU and SD-card evidence in tmpfs; this module
fetches it, reads the published catalog's output digests, and reports phase
times. A baseline run supplies the "identical output" gate for an experiment.
"""

import hashlib
import json
import time
from pathlib import Path

PROFILE_ID = "catalog-fresh-build"
EVIDENCE_FILES = ("timeline.tsv", "tasks.tsv", "stacks.tsv", "maps.txt", "marks.tsv")
DIGESTS = (
    "identity_sha256",
    "ordering_sha256",
    "launch_sha256",
    "search_sha256",
    "artifact_set_sha256",
)
# Each phase ends at its milestone and starts where the previous present phase
# ended, from the build's start. A card without Arcade folds that phase into
# the scan rather than losing it.
PHASES = (
    ("start to Arcade ready", "system-prepared:arcade"),
    ("scan other systems", "plan-ready"),
    ("build search DBs and NavPacks", "saving-systems"),
    ("copy artifacts to SD", "saving-metadata"),
    ("metadata and publish", "published"),
)
EVIDENCE_TIMEOUT_SECONDS = 900


def read_table(text: str) -> list[dict[str, str]]:
    # Not `splitlines`: logged file names may contain other control characters.
    lines = text.split("\n")
    header = lines[0].split("\t")
    rows = (line.split("\t") for line in lines[1:] if line)
    return [dict(zip(header, cells)) for cells in rows if len(cells) == len(header)]


def milestones(marks: str) -> dict[str, int]:
    """First time of each milestone, in milliseconds from the build's start."""
    found: dict[str, int] = {}
    for row in read_table(marks):
        found.setdefault(row["name"], int(row["t_ms"]))
    return found


def catalog_digests(inspect: str) -> dict[str, str]:
    """The published catalog's identity: game count and output digests."""
    summaries = [
        line
        for line in inspect.splitlines()
        if line.startswith("catalog_v3_summary_tsv\t")
    ]
    if len(summaries) != 1:
        raise ValueError("catalog inspection has no single summary line")
    fields = dict(part.split("=", 1) for part in summaries[0].split("\t")[1:])
    if fields.get("valid") != "1":
        raise ValueError("installed catalog is not valid")
    return {key: fields[key] for key in ("total_games", *DIGESTS)}


def differing_digests(baseline: dict[str, str], current: dict[str, str]) -> list[str]:
    return [key for key in baseline if baseline[key] != current.get(key)]


def phase_report(evidence: dict, timeline: str, marks: dict[str, int]):
    """CPU, SD-card traffic and elapsed time between consecutive milestones."""
    rows = [
        {key: int(value) for key, value in row.items()} for row in read_table(timeline)
    ]
    hertz = evidence["clock_ticks_per_second"]

    def cpu_percent(change, user, system):
        ticks = change[user] + change[system]
        return round(100 * ticks * 1000 / hertz / max(change["t_ms"], 1))

    def at(milestone):
        moment = 0 if milestone is None else marks[milestone]
        return min(rows, key=lambda row: abs(row["t_ms"] - moment))

    report = []
    begin = None
    for name, end in PHASES:
        if end not in marks:
            continue
        first, last = at(begin), at(end)
        begin = end
        change = {key: last[key] - first[key] for key in first}
        wall = max(change["t_ms"], 1)
        cpu_total = sum(
            change[key]
            for key in (
                "cpu_user",
                "cpu_nice",
                "cpu_system",
                "cpu_idle",
                "cpu_iowait",
                "cpu_irq",
                "cpu_softirq",
            )
        )
        busy = cpu_total - change["cpu_idle"] - change["cpu_iowait"]
        report.append(
            {
                "phase": name,
                "start_ms": first["t_ms"],
                "wall_ms": wall,
                "build_cpu_percent": cpu_percent(change, "build_utime", "build_stime"),
                "other_threads_cpu_percent": cpu_percent(change, "utime", "stime")
                - cpu_percent(change, "build_utime", "build_stime"),
                "all_cpus_busy_percent": round(100 * busy / cpu_total)
                if cpu_total
                else 0,
                # Request service time, capped where requests overlap: this
                # kernel's `io_ticks` stays zero.
                "sd_busy_percent": min(
                    100,
                    round(
                        100 * (change["disk_read_ms"] + change["disk_write_ms"]) / wall
                    ),
                ),
                "sd_read_mib": round(change["disk_read_sectors"] / 2048, 1),
                "sd_write_mib": round(change["disk_write_sectors"] / 2048, 1),
            }
        )
    return report


def wait_for_evidence(agent, *, sleep=time.sleep, clock=time.monotonic) -> bytes:
    """Poll for the run's completion marker, which the device writes last."""
    from .client import AgentError

    deadline = clock() + EVIDENCE_TIMEOUT_SECONDS
    while True:
        try:
            return agent.read_profile_artifact(PROFILE_ID, "evidence.json")
        except AgentError as error:
            if not str(error).startswith("artifact-unavailable"):
                raise
            if clock() >= deadline:
                raise AgentError(
                    "no cold-build evidence after "
                    f"{EVIDENCE_TIMEOUT_SECONDS} s; the installed application must be "
                    "the Development tooling build and the catalog must have been purged"
                ) from error
            sleep(5)


def default_symbols() -> Path:
    from .apps import application
    from .build import TARGET
    from .cli import repository

    app = application("magik")
    return repository() / app.package / "target" / TARGET / app.profile / app.binary


def profile_cpu(arguments, agent, run, evidence, files):
    """Symbolised CPU and blocked-time attribution, when the executable matches."""
    from .catalog_cold_profile import (
        Symboliser,
        blocked_by_path,
        fold_stacks,
        rank_functions,
    )

    path = arguments.symbols or default_symbols()
    if not path.is_file():
        return {"skipped": f"no executable at {path}"}
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    installed = agent.status().fields.get("artifacts", {}).get("magik")
    if digest != installed:
        return {"skipped": f"{path} is not the installed executable ({installed})"}
    executable = evidence.get("executable")
    if not executable:
        return {"skipped": "evidence names no executable"}
    symboliser = Symboliser.from_files(executable, files["maps.txt"], path)
    tasks = files["tasks.tsv"]
    names = {row["tid"]: row["comm"] for row in read_table(tasks)}
    folded = fold_stacks(files["stacks.tsv"], names, symboliser)
    (run / "cpu.folded").write_text(
        "".join(f"{stack} {count}\n" for stack, count in folded.most_common())
    )
    return {
        "cpu": rank_functions(folded, 25),
        "blocked": blocked_by_path(tasks, evidence["task_interval_ms"], 25),
    }


def cold_build(arguments, run):
    from .cli import connect_agent
    from .device import catalog_operation, reboot_device
    from .results import append_event

    agent, _ = connect_agent(
        run, {"catalog-operations-v1", "artifacts-v1", "device-control-v1"}
    )
    catalog_operation(
        agent, run, {"action": "purge", "layout": "dev", "confirm": True}, "purge"
    )
    append_event(run, {"phase": "cold-build", "outcome": "purged"})
    # One reboot, never replayed: a lost acknowledgement is reconciled by hand.
    reboot_device(arguments, run, agent=agent)
    evidence = json.loads(wait_for_evidence(agent))
    files = {
        name: agent.read_profile_artifact(PROFILE_ID, name).decode(errors="replace")
        for name in EVIDENCE_FILES
    }
    (run / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    for name, text in files.items():
        (run / name).write_text(text)
    if evidence["outcome"] != "published":
        raise RuntimeError(f"fresh catalog build {evidence['outcome']}; see {run}")
    _, output = catalog_operation(
        agent, run, {"action": "inspect", "layout": "dev"}, "inspect"
    )
    inspect = output.decode(errors="replace")
    marks = milestones(files["marks.tsv"])
    summary = {
        "outcome": evidence["outcome"],
        "wall_ms": evidence["wall_ms"],
        "kernel": evidence["kernel"],
        "milestones_ms": marks,
        "phases": phase_report(evidence, files["timeline.tsv"], marks),
        "catalog": catalog_digests(inspect),
    }
    if arguments.baseline:
        baseline = json.loads((arguments.baseline / "summary.json").read_text())
        summary["baseline"] = str(arguments.baseline)
        summary["baseline_wall_ms"] = baseline["wall_ms"]
        summary["output_differs"] = differing_digests(
            baseline["catalog"], summary["catalog"]
        )
    summary["profile"] = profile_cpu(arguments, agent, run, evidence, files)
    (run / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    append_event(
        run,
        {"phase": "cold-build", "outcome": "published", "wall_ms": summary["wall_ms"]},
    )
    print(render(summary))
    return 1 if summary.get("output_differs") else 0


def render(summary: dict) -> str:
    title = (
        f"cold build {summary['outcome']} in {summary['wall_ms'] / 1000:.1f} s; "
        + f"{summary['catalog']['total_games']} games"
    )
    header = " ".join(
        [
            f"{'phase':32s}",
            f"{'wall s':>7s}",
            f"{'build%':>6s}",
            f"{'others%':>7s}",
            f"{'cpus%':>5s}",
            f"{'SD busy%':>8s}",
            f"{'read MiB':>8s}",
            f"{'write MiB':>9s}",
        ]
    )
    lines = [title, header]
    for phase in summary["phases"]:
        lines.append(
            f"{phase['phase']:32s} {phase['wall_ms'] / 1000:7.1f} "
            f"{phase['build_cpu_percent']:6d} {phase['other_threads_cpu_percent']:7d} "
            f"{phase['all_cpus_busy_percent']:5d} {phase['sd_busy_percent']:8d} "
            f"{phase['sd_read_mib']:8.1f} {phase['sd_write_mib']:9.1f}"
        )
    if "baseline" in summary:
        change = summary["wall_ms"] - summary["baseline_wall_ms"]
        verdict = (
            "OUTPUT DIFFERS: " + ", ".join(summary["output_differs"])
            if summary["output_differs"]
            else "output identical"
        )
        lines.append(f"vs baseline: {change / 1000:+.1f} s; {verdict}")
    return "\n".join(lines)
