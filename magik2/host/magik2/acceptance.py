"""Bounded, reproducible delivery measurements; results never gate deployment."""

from __future__ import annotations

import json
import os
import subprocess
import time
from pathlib import Path


def summarize(attempts, target_ms):
    durations = sorted(row["elapsed_ms"] for row in attempts)
    failures = sum(row["exit_code"] != 0 for row in attempts)
    slowest = max(durations, default=None)
    return {
        "attempts": len(attempts),
        "failures": failures,
        "slowest_ms": slowest,
        "target_ms": target_ms,
        "target_met": len(durations) == 2 and not failures and slowest <= target_ms,
    }


def run_delivery_matrix(run: Path) -> int:
    """Two attempts per case, including failures, then restore original sources/app."""
    repository = Path(__file__).resolve().parents[3]
    probe = repository / "magik2/probe"
    rust = probe / "src/main.rs"
    slint = probe / "ui/probe.slint"
    originals = {path: path.read_bytes() for path in (rust, slint)}
    artifact = probe / "target/armv7-unknown-linux-gnueabihf/release/mini-magik"
    index = {"cases": {}, "restoration": None}
    interrupted = False
    base_env = dict(os.environ)
    base_env.pop("MISTER_MAGIK2_REPAIR", None)
    base_env.pop("MISTER_MAGIK2_PREBUILT_ARTIFACT", None)

    def save():
        (run / "acceptance.json").write_text(json.dumps(index, indent=2) + "\n")

    def attempt(case, number, extra=None):
        nonlocal interrupted
        folder = run / case / str(number)
        folder.mkdir(parents=True)
        env = {
            **base_env,
            "MISTER_MAGIK2_RESULTS": str(folder.resolve()),
            **(extra or {}),
        }
        started = time.monotonic()
        with (folder / "command.log").open("w") as output:
            try:
                result = subprocess.run(
                    [str(repository / "scripts/magik2"), "deploy"],
                    cwd=repository,
                    env=env,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    timeout=120,
                )
                code = result.returncode
            except subprocess.TimeoutExpired:
                code = 124
            except KeyboardInterrupt:
                code = 130
                interrupted = True
        bundles = list(folder.glob("*/run.json"))
        return {
            "number": number,
            "exit_code": code,
            "elapsed_ms": round((time.monotonic() - started) * 1000),
            "bundle": str(bundles[0].parent.relative_to(run))
            if len(bundles) == 1
            else None,
        }

    try:
        index["warmup"] = attempt("warmup", 0)
        save()
        if index["warmup"]["exit_code"]:
            return 1
        original_artifact = artifact.read_bytes()
        for case, target in (
            ("no-op", 1000),
            ("prebuilt", 5000),
            ("rust-edit", 15000),
            ("slint-edit", 15000),
        ):
            rows = []
            index["cases"][case] = {"attempts": rows}
            for number in range(2):
                extra = {}
                if case == "prebuilt":
                    payload = run / "changed-probe"
                    payload.write_bytes(
                        original_artifact + f"\nacceptance {number}\n".encode()
                    )
                    extra["MISTER_MAGIK2_PREBUILT_ARTIFACT"] = str(payload.resolve())
                elif case == "rust-edit":
                    rust.write_bytes(
                        originals[rust].replace(
                            b"magik2-probe ready width=",
                            f"magik2-probe sample-{number} ready width=".encode(),
                            1,
                        )
                    )
                elif case == "slint-edit":
                    slint.write_bytes(
                        originals[slint].replace(
                            b'MiSTer MagiK 2 Probe"',
                            f'MiSTer MagiK 2 Probe {number}"'.encode(),
                            1,
                        )
                    )
                rows.append(attempt(case, number, extra))
                index["cases"][case]["summary"] = summarize(rows, target)
                save()
                if interrupted:
                    raise KeyboardInterrupt
            for path, original in originals.items():
                path.write_bytes(original)
            print(f"{case}: {index['cases'][case]['summary']}", flush=True)
    finally:
        for path, original in originals.items():
            path.write_bytes(original)
        (run / "changed-probe").unlink(missing_ok=True)
        index["restoration"] = attempt("restoration", 0)
        save()
    return int(
        index["restoration"]["exit_code"] != 0
        or any(case["summary"]["failures"] for case in index["cases"].values())
    )


def run_contract_checks(run: Path) -> int:
    """Exercise native recovery without altering Main, platform files or rebooting."""
    from .cli import connect_agent, ensure_application, CHECK_AGENT_CAPABILITIES
    from .client import AgentError
    from .frames import decode_preview
    from .results import append_event, retain_diagnostics

    agent, status = connect_agent(
        run, CHECK_AGENT_CAPABILITIES | {"watch-v1", "diagnostics"}
    )
    ensure_application(agent, status, run)
    expected = agent.expected_sha256

    def record(case, **evidence):
        append_event(
            run, {"phase": "contract", "case": case, "outcome": "passed", **evidence}
        )

    try:
        before = dict(agent.status().fields)
        # Simulate a fresh checkout's absent credential cache using the fixed
        # token discovery adapter, then return to the original shared cache.
        import tempfile

        repository = Path(__file__).resolve().parents[3]
        with tempfile.TemporaryDirectory(prefix="magik2-token-check-") as credentials:
            folder = run / "fresh-credentials"
            folder.mkdir()
            env = {
                **os.environ,
                "MISTER_MAGIK2_STATE": credentials,
                "MISTER_MAGIK2_RESULTS": str(folder.resolve()),
            }
            with (folder / "command.log").open("w") as output:
                result = subprocess.run(
                    [str(repository / "scripts/magik2"), "status"],
                    cwd=repository,
                    env=env,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    timeout=30,
                )
            assert result.returncode == 0, "fresh credential discovery failed"
        retained = agent.status().fields
        assert (
            retained["agent_pid"] == before["agent_pid"]
            and retained["pid"] == before["pid"]
        )
        record(
            "fresh credential cache retains compatible agent and probe",
            agent_pid=retained["agent_pid"],
            probe_pid=retained["pid"],
        )
        reply, _ = agent._request(
            "upload", {"artifact": "probe", "sha256": "0" * 64}, b"invalid probe"
        )
        assert reply.operation == "error", "bad hash was accepted"
        assert agent.status().fields["running_sha256"] == expected
        record("bad upload preserves running artifact", error=dict(reply.fields))
        try:
            agent.start(expected_sha256="0" * 64)
        except AgentError as error:
            assert "artifact-superseded" in str(error)
        else:
            raise AssertionError("superseded artifact started")
        after = agent.status().fields
        assert (
            after["pid"] == before["pid"]
            and after["running_sha256"] == before["running_sha256"]
        )
        record("superseded start preserves process")
        for cycle in range(2):
            with agent.open_watch() as connection:
                connection.settimeout(5)
                kinds = set()
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline and len(kinds) < 3:
                    event, body = agent.read_watch_event(connection)
                    kinds.add(event.operation)
                    if event.operation == "watch-frame":
                        decode_preview(body)
                assert kinds == {"watch-frame", "watch-log", "watch-metrics"}, kinds
            record("watch reconnect", cycle=cycle, events=sorted(kinds))
        with agent.open_watch() as connection:
            time.sleep(2)
            started = time.monotonic()
            assert agent.status().fields["ready"]
            elapsed = round((time.monotonic() - started) * 1000)
            assert elapsed < 2000, elapsed
            record("stalled viewer leaves control responsive", elapsed_ms=elapsed)
        # Exercise the real native preview producer during the same pytest workload.
        from .viewer import serve

        server, _url = serve(agent)
        try:
            repository = Path(__file__).resolve().parents[3]
            folder = run / "viewer-motion"
            folder.mkdir()
            env = {**os.environ, "MISTER_MAGIK2_RESULTS": str(folder.resolve())}
            with (folder / "command.log").open("w") as output:
                result = subprocess.run(
                    [str(repository / "scripts/magik2"), "check", "motion"],
                    cwd=repository,
                    env=env,
                    stdout=output,
                    stderr=subprocess.STDOUT,
                    timeout=150,
                )
            assert result.returncode == 0, (
                "motion failed with active native viewer; see viewer-motion"
            )
            record(
                "five motion repetitions with active viewer",
                bundles=[
                    str(p.parent.relative_to(run)) for p in folder.glob("*/run.json")
                ],
            )
        finally:
            server.server_close()
        tunnel = agent.open_test_tunnel()
        test_pid = agent.status().fields.get("pid")
        tunnel.close()
        deadline = time.monotonic() + 35
        while time.monotonic() < deadline:
            restored = agent.status().fields
            if restored.get("ready") and restored.get("pid") != test_pid:
                break
            time.sleep(0.2)
        else:
            raise AssertionError(
                "disconnected session did not restore persistent probe"
            )
        assert restored["running_sha256"] == expected
        record(
            "partial attachment restores persistent probe",
            test_pid=test_pid,
            restored=dict(restored),
        )
        stopped = agent.stop()
        device = agent.diagnostics()
        assert stopped["launcher_resumed"] and not agent.status().fields["running"]
        record("stop observes launcher recovery", diagnostics=device)
    finally:
        agent.start(expected_sha256=expected)
        retain_diagnostics(run, agent)
    return 0
