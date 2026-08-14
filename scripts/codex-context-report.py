#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Report aggregate Codex context growth without exposing session content."""

from __future__ import annotations

import argparse
from collections import defaultdict
import json
import math
import os
from pathlib import Path
import re
import sys
from typing import Any, Iterable


SCHEMA_VERSION = 1
LARGE_OUTPUT_BYTES = 10 * 1024
NESTED_TOOL = re.compile(r"\btools\.([A-Za-z_][A-Za-z0-9_]*)\s*\(")
KNOWN_RECORD_TYPES = {
    "compacted",
    "event_msg",
    "response_item",
    "session_meta",
    "turn_context",
    "world_state",
}


def output_size(value: Any) -> int:
    """Return model-visible UTF-8 bytes without retaining the rendered value."""
    if isinstance(value, str):
        return len(value.encode("utf-8"))
    if isinstance(value, list):
        total = 0
        for item in value:
            if isinstance(item, dict) and isinstance(item.get("text"), str):
                total += len(item["text"].encode("utf-8"))
            else:
                total += len(
                    json.dumps(item, ensure_ascii=False, separators=(",", ":")).encode(
                        "utf-8"
                    )
                )
        return total
    return len(
        json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    )


def tool_name(payload: dict[str, Any]) -> str:
    name = payload.get("name")
    if not isinstance(name, str) or not name:
        return "unknown"
    if name != "exec":
        return name
    source = payload.get("input")
    if not isinstance(source, str):
        return "exec"
    nested = sorted(set(NESTED_TOOL.findall(source)))
    if len(nested) == 1:
        return nested[0]
    if nested:
        return f"exec[{'+'.join(nested)}]"
    return "exec"


def warning_list(counts: dict[str, int]) -> list[dict[str, int | str]]:
    return [
        {"code": code, "count": count}
        for code, count in sorted(counts.items())
        if count
    ]


def analyze_sessions(paths: Iterable[Path]) -> dict[str, Any]:
    calls: dict[tuple[int, str], str] = {}
    pending_outputs: list[tuple[int, str, int]] = []
    tool_sizes: dict[str, list[int]] = defaultdict(list)
    input_samples: list[int] = []
    context_windows: list[int] = []
    warning_counts: dict[str, int] = defaultdict(int)
    compactions = 0
    sessions = 0

    for session_index, path in enumerate(paths):
        top_level_compactions = 0
        event_compactions = 0
        try:
            handle = path.open("r", encoding="utf-8")
        except OSError:
            warning_counts["unreadable_sessions"] += 1
            continue
        sessions += 1
        with handle:
            for line in handle:
                try:
                    record = json.loads(line)
                except (json.JSONDecodeError, UnicodeDecodeError):
                    warning_counts["malformed_lines"] += 1
                    continue
                if not isinstance(record, dict):
                    warning_counts["unknown_records"] += 1
                    continue
                record_type = record.get("type")
                if record_type not in KNOWN_RECORD_TYPES:
                    warning_counts["unknown_records"] += 1
                    continue
                payload = record.get("payload")
                if not isinstance(payload, dict):
                    continue

                if record_type == "compacted":
                    top_level_compactions += 1
                    continue
                if record_type == "session_meta":
                    window = payload.get("context_window")
                    if isinstance(window, int):
                        context_windows.append(window)
                    continue
                if record_type == "event_msg":
                    event_type = payload.get("type")
                    if event_type == "context_compacted":
                        event_compactions += 1
                    elif event_type == "task_started":
                        window = payload.get("model_context_window")
                        if isinstance(window, int):
                            context_windows.append(window)
                    elif event_type == "token_count":
                        info = payload.get("info")
                        if isinstance(info, dict):
                            window = info.get("model_context_window")
                            if isinstance(window, int):
                                context_windows.append(window)
                            usage = info.get("last_token_usage")
                            if isinstance(usage, dict):
                                input_tokens = usage.get("input_tokens")
                                if isinstance(input_tokens, int):
                                    input_samples.append(input_tokens)
                    continue
                if record_type != "response_item":
                    continue

                item_type = payload.get("type")
                call_id = payload.get("call_id")
                if item_type in {"function_call", "custom_tool_call"}:
                    if isinstance(call_id, str):
                        calls[(session_index, call_id)] = tool_name(payload)
                    continue
                if item_type not in {"function_call_output", "custom_tool_call_output"}:
                    continue
                if not isinstance(call_id, str) or "output" not in payload:
                    warning_counts["unmatched_outputs"] += 1
                    continue
                pending_outputs.append(
                    (session_index, call_id, output_size(payload["output"]))
                )
        compactions += top_level_compactions or event_compactions

    for session_index, call_id, size in pending_outputs:
        name = calls.get((session_index, call_id))
        if name is None:
            warning_counts["unmatched_outputs"] += 1
            name = "unknown"
        tool_sizes[name].append(size)

    tools = []
    for name, sizes in tool_sizes.items():
        total_bytes = sum(sizes)
        tools.append(
            {
                "tool": name,
                "calls": len(sizes),
                "output_bytes": total_bytes,
                "estimated_tokens": math.ceil(total_bytes / 4),
                "average_bytes": round(total_bytes / len(sizes)),
                "largest_bytes": max(sizes),
                "over_10_kib": sum(size > LARGE_OUTPUT_BYTES for size in sizes),
            }
        )
    tools.sort(key=lambda row: (-row["estimated_tokens"], row["tool"]))

    context = {
        "samples": len(input_samples),
        "first_input_tokens": input_samples[0] if input_samples else None,
        "maximum_input_tokens": max(input_samples) if input_samples else None,
        "final_input_tokens": input_samples[-1] if input_samples else None,
        "maximum_context_window": max(context_windows) if context_windows else None,
        "compactions": compactions,
    }
    return {
        "version": SCHEMA_VERSION,
        "sessions": sessions,
        "context": context,
        "tools": tools,
        "warnings": warning_list(warning_counts),
    }


def recent_sessions(count: int) -> list[Path]:
    codex_home = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
    sessions_root = codex_home / "sessions"
    candidates: list[tuple[float, Path]] = []
    try:
        for path in sessions_root.rglob("*.jsonl"):
            try:
                candidates.append((path.stat().st_mtime, path))
            except OSError:
                continue
    except OSError:
        return []
    candidates.sort(key=lambda item: item[0], reverse=True)
    return [path for _, path in reversed(candidates[:count])]


def format_number(value: int | None) -> str:
    return "unavailable" if value is None else f"{value:,}"


def render_human(report: dict[str, Any]) -> str:
    context = report["context"]
    lines = [
        "Codex context report",
        f"Sessions: {report['sessions']}",
        (
            "Context: "
            f"first={format_number(context['first_input_tokens'])} "
            f"maximum={format_number(context['maximum_input_tokens'])} "
            f"final={format_number(context['final_input_tokens'])} "
            f"window={format_number(context['maximum_context_window'])} "
            f"compactions={context['compactions']}"
        ),
        "",
        "Tool | Calls | Output bytes | Est. tokens | Avg bytes | Largest bytes | >10 KiB",
        "--- | ---: | ---: | ---: | ---: | ---: | ---:",
    ]
    if report["tools"]:
        for row in report["tools"]:
            lines.append(
                f"{row['tool']} | {row['calls']:,} | {row['output_bytes']:,} | "
                f"{row['estimated_tokens']:,} | {row['average_bytes']:,} | "
                f"{row['largest_bytes']:,} | {row['over_10_kib']:,}"
            )
    else:
        lines.append("(no tool outputs found) | 0 | 0 | 0 | 0 | 0 | 0")
    if report["warnings"]:
        summary = ", ".join(
            f"{warning['code']}={warning['count']}" for warning in report["warnings"]
        )
        lines.extend(["", f"Warnings: {summary}"])
    return "\n".join(lines)


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sessions", nargs="*", type=Path, metavar="SESSION.jsonl")
    parser.add_argument("--recent", type=int, default=None, metavar="N")
    parser.add_argument("--json", action="store_true", dest="json_output")
    args = parser.parse_args(argv)
    if args.sessions and args.recent is not None:
        parser.error("explicit sessions and --recent are mutually exclusive")
    if args.recent is not None and args.recent < 1:
        parser.error("--recent must be positive")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    paths = args.sessions or recent_sessions(args.recent or 1)
    if not paths or any(not path.is_file() for path in paths):
        print("codex_context_report_error: no readable session input", file=sys.stderr)
        return 2
    report = analyze_sessions(paths)
    if report["sessions"] == 0:
        print("codex_context_report_error: no readable session input", file=sys.stderr)
        return 2
    if args.json_output:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(render_human(report))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
