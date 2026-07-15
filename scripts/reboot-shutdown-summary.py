#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Summarize MiSTer MagiK reboot shutdown evidence."""

from __future__ import annotations

import argparse
import csv
import re
from collections import defaultdict
from pathlib import Path
from statistics import mean


def parse_int(value: str) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def percentile(values: list[int], pct: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    index = round((len(ordered) - 1) * pct)
    return ordered[index]


def fmt_ms(value: int | float | None) -> str:
    if value is None:
        return "n/a"
    return f"{value:.0f}ms"


def load_agent_rows(path: Path, samples: int) -> list[dict[str, str]]:
    if not path.exists():
        return []
    with path.open(newline="") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    return rows[-samples:]


def summarize_agent(rows: list[dict[str, str]]) -> None:
    print("Agent Boot Profile")
    if not rows:
        print("  no rows found")
        return

    by_mode: dict[str, list[int]] = defaultdict(list)
    recovered_by_mode: dict[str, int] = defaultdict(int)
    legacy_recovered_by_mode: dict[str, int] = defaultdict(int)
    rx_stalls = []
    for row in rows:
        mode = row.get("mode", "")
        down_ms = parse_int(row.get("down_ms", ""))
        if down_ms is not None:
            by_mode[mode].append(down_ms)
        note = row.get("note", "")
        base_recovered = (
            row.get("down_ms")
            and row.get("agent_ready_ms")
            and row.get("ssh_exec_ready_ms")
            and "launcher_state=LauncherActive" in note
        )
        if base_recovered and "agent_rx_nonzero=1" in note and "agent_rx_increasing=1" in note:
            recovered_by_mode[mode] += 1
        elif (
            base_recovered
            and "agent_rx_nonzero=" not in note
            and "agent_rx_increasing=" not in note
        ):
            legacy_recovered_by_mode[mode] += 1
        if "agent_rx_nonzero=0" in note or "agent_rx_increasing=0" in note:
            rx_stalls.append(row)

    for mode in sorted(by_mode):
        values = by_mode[mode]
        print(
            f"  {mode}: n={len(values)} rx_verified_recovered={recovered_by_mode[mode]} "
            f"legacy_recovered={legacy_recovered_by_mode[mode]} "
            f"min={fmt_ms(min(values))} avg={fmt_ms(mean(values))} "
            f"p95={fmt_ms(percentile(values, 0.95))} max={fmt_ms(max(values))}"
        )

    worst = sorted(
        (row for row in rows if parse_int(row.get("down_ms", "")) is not None),
        key=lambda row: parse_int(row.get("down_ms", "")) or 0,
        reverse=True,
    )[:5]
    if worst:
        print("  worst shutdown observations:")
        for row in worst:
            print(
                "    sample={sample} mode={mode} down={down} agent={agent} ssh={ssh} note={note}".format(
                    sample=row.get("sample", ""),
                    mode=row.get("mode", ""),
                    down=fmt_ms(parse_int(row.get("down_ms", ""))),
                    agent=fmt_ms(parse_int(row.get("agent_ready_ms", ""))),
                    ssh=fmt_ms(parse_int(row.get("ssh_exec_ready_ms", ""))),
                    note=row.get("note", "")[:160],
                )
            )
    if rx_stalls:
        print(f"  RX-stall candidates: {len(rx_stalls)}")


MAIN_RE = re.compile(r"^(?P<ms>\d+) pid=(?P<pid>\d+) .* stage=(?P<stage>\S+)")


def summarize_main_log(path: Path, samples: int) -> list[dict[str, object]]:
    print("\nMain Reboot Breadcrumbs")
    if not path.exists():
        print("  no log found")
        return []
    groups: list[dict[str, object]] = []
    current: dict[str, object] | None = None
    for line in path.read_text(errors="replace").splitlines():
        match = MAIN_RE.search(line)
        if not match:
            continue
        stage = match.group("stage")
        if stage == "requested" or current is None:
            current = {"pid": match.group("pid"), "stages": {}}
            groups.append(current)
        stages = current["stages"]
        assert isinstance(stages, dict)
        stages[stage] = int(match.group("ms"))

    recent = groups[-samples:]
    if not recent:
        print("  no parseable breadcrumbs")
        return []
    for group in recent:
        pid = group["pid"]
        stages = group["stages"]
        assert isinstance(stages, dict)
        requested = stages.get("requested")
        finished = stages.get("linux_reboot_spawned")
        finish_label = "spawn"
        if finished is None:
            finished = stages.get("direct_reset_start")
            finish_label = "direct"
        sync_start = stages.get("sync_start")
        sync_done = stages.get("sync_done")
        print(
            f"  pid={pid} request_to_{finish_label}={fmt_ms((finished - requested) if requested is not None and finished is not None else None)} "
            f"sync={fmt_ms((sync_done - sync_start) if sync_start is not None and sync_done is not None else None)}"
        )
    return recent


TRACE_DONE_RE = re.compile(
    r"done step=(?P<step>\S+)(?: service=(?P<service>\S+))? rc=(?P<rc>\d+) elapsed_s=(?P<elapsed>[0-9.]+)"
)
TRACE_RCK_START_RE = re.compile(r"^(?P<uptime>[0-9.]+) shutdown-trace start step=rcK-deep")


def parse_shutdown_groups(path: Path) -> list[dict[str, object]]:
    if not path.exists():
        return []
    groups: list[dict[str, object]] = []
    current: dict[str, object] | None = None
    for line in path.read_text(errors="replace").splitlines():
        start = TRACE_RCK_START_RE.search(line)
        if start:
            current = {
                "rck_start_ms": round(float(start.group("uptime")) * 1000),
                "services": {},
            }
            groups.append(current)
        if current is None:
            continue
        done = TRACE_DONE_RE.search(line)
        if not done:
            continue
        step = done.group("step")
        service = done.group("service")
        elapsed_ms = round(float(done.group("elapsed")) * 1000)
        rc = int(done.group("rc"))
        if step == "rcK.service" and service:
            services = current["services"]
            assert isinstance(services, dict)
            services[Path(service).name] = (elapsed_ms, rc)
        else:
            current[step] = (elapsed_ms, rc)
    return groups


def summarize_shutdown_trace(
    path: Path, samples: int, agent_rows: list[dict[str, str]], main_groups: list[dict[str, object]]
) -> None:
    print("\nShutdown Trace")
    groups = parse_shutdown_groups(path)
    if not groups:
        print("  no log found")
        return
    recent = groups[-samples:]
    if agent_rows and main_groups and len(recent) == len(agent_rows) == len(main_groups):
        print("  joined recent samples:")
        print("    sample down main_spawn_to_rcK rcK S99user S99_rc swapoff umount")
        for agent, main, trace in zip(agent_rows, main_groups, recent):
            stages = main["stages"]
            assert isinstance(stages, dict)
            spawn = stages.get("linux_reboot_spawned")
            rck_start_ms = trace.get("rck_start_ms")
            spawn_to_rck = (
                int(rck_start_ms) - int(spawn)
                if spawn is not None and rck_start_ms is not None
                else None
            )
            services = trace["services"]
            assert isinstance(services, dict)
            s99_elapsed, s99_rc = services.get("S99user", (None, None))
            rck_elapsed, _ = trace.get("rcK-deep", (None, None))
            swapoff_elapsed, _ = trace.get("swapoff", (None, None))
            umount_elapsed, _ = trace.get("umount", (None, None))
            print(
                "    {sample} {down} {spawn_to_rck} {rck} {s99} {s99_rc} {swapoff} {umount}".format(
                    sample=agent.get("sample", ""),
                    down=fmt_ms(parse_int(agent.get("down_ms", ""))),
                    spawn_to_rck=fmt_ms(spawn_to_rck),
                    rck=fmt_ms(rck_elapsed),
                    s99=fmt_ms(s99_elapsed),
                    s99_rc=s99_rc if s99_rc is not None else "n/a",
                    swapoff=fmt_ms(swapoff_elapsed),
                    umount=fmt_ms(umount_elapsed),
                )
            )

    durations = []
    for group in recent:
        services = group["services"]
        assert isinstance(services, dict)
        for service, (elapsed_ms, rc) in services.items():
            durations.append((elapsed_ms, service, rc))
        for step in ["rcK-deep", "swapoff", "umount"]:
            if step in group:
                elapsed_ms, rc = group[step]
                durations.append((elapsed_ms, step, rc))
    if not durations:
        print("  no timed deep-trace rows found")
        return
    print("  slowest recent trace rows:")
    for elapsed_ms, service, rc in sorted(durations, reverse=True)[:12]:
        print(f"    {elapsed_ms / 1000:6.2f}s rc={rc} {service}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--agent-tsv",
        default="history/toolchain-bench/results-agent.tsv",
        type=Path,
    )
    parser.add_argument(
        "--main-log",
        default="build/agent-diagnostics/latest/main-reboot.log",
        type=Path,
    )
    parser.add_argument(
        "--shutdown-log",
        default="build/agent-diagnostics/latest/shutdown-trace.log",
        type=Path,
    )
    parser.add_argument("--samples", type=int, default=30)
    args = parser.parse_args()

    rows = load_agent_rows(args.agent_tsv, args.samples)
    summarize_agent(rows)
    main_groups = summarize_main_log(args.main_log, args.samples)
    summarize_shutdown_trace(args.shutdown_log, args.samples, rows, main_groups)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
