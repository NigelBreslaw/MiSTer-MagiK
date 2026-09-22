"""One explicitly selected concept using the existing native Slint test session."""
from __future__ import annotations

import json
import math
import time
import uuid
from pathlib import Path
from .testing import fresh_session, one_element
from .results import append_event
from .capture import capture_png

EFFECTS = ("raster-waves","texture-tunnel","palette-aurora","starfield-comets","pixel-dissolve","mirror-floor","light-sweep","depth-parallax","point-cloud-morph","diagnostic",)
PRESETS = ("default", "reduced")

def value(application, name):
    return one_element(application, "concept-" + name).accessible_value

def wait_for(predicate, message, timeout=15):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise RuntimeError(message)
        time.sleep(0.025)

def action(application, name):
    one_element(application, "concept-" + name).invoke_accessible_default_action()

def select(application, effect, preset):
    if effect not in EFFECTS or preset not in PRESETS:
        raise ValueError("unsupported concept or preset")
    action(application, f"select-{effect}-{preset}")
    wait_for(lambda: value(application, "name") == effect or bool(value(application, "error")), "concept preparation timed out")
    if error := value(application, "error"):
        raise RuntimeError(error)

def capture(agent, destination):
    fields, pixels = agent.capture_framebuffer()
    png, metadata = capture_png(fields, pixels, "raw")
    destination.write_bytes(png)
    destination.with_suffix(".json").write_text(json.dumps(metadata, indent=2) + "\n")

def validate(metrics, sha256, effect, preset, profile=False):
    if metrics.get("sha256") != sha256:
        raise ValueError("concept artifact identity mismatch")
    w = metrics.get("window")
    if not isinstance(w, dict):
        raise ValueError("missing concept measurement window")
    duration = 10_000 if profile else 30_000
    if w.get("instrumented") is not profile or not duration <= w.get("elapsed_ms", 0) <= duration + 1000:
        raise ValueError("invalid concept measurement boundaries")
    if w.get("end_ms", 0) - w.get("start_ms", 0) != w["elapsed_ms"]:
        raise ValueError("inconsistent concept clock")
    context = w.get("context", {})
    if (context.get("concept"), context.get("preset"), context.get("route")) != (effect, preset, "hdmi"):
        raise ValueError("concept configuration mismatch")
    cpu = w.get("process_cpu_percent")
    rss = w.get("peak_rss_bytes")
    refresh = w.get("refresh_hz")
    if not all(type(x) in (int, float) and math.isfinite(x) and x > 0 for x in (cpu, rss, refresh)):
        raise ValueError("CPU, RSS or refresh evidence unavailable")
    if w.get("evidence_error") or not w.get("drop_baseline_available"):
        raise ValueError("invalid physical evidence")
    n = w.get("presentations", 0)
    cadence = n > 0 and all(w.get(k) == n for k in ("physical_latch_posts", "physical_latch_flips", "presented_vblanks", "owned_vblanks"))
    clean = all(w.get(k) == 0 for k in ("physical_drops", "latch_drops", "latch_rejections"))
    passed = cadence and clean and 59 <= refresh <= 61 and cpu < 150 and rss <= 128 * 1024 * 1024
    return {**w, "sha256": sha256, "fps": n * 1000 / w["elapsed_ms"], "qualified": passed and not profile, "instrumented": profile}

def measure(application, agent, run, effect, preset, profile):
    results = []
    for repetition in range(1 if profile else 2):
        select(application, effect, preset)
        action(application, "measure")
        wait_for(lambda: value(application, "measuring") == "true", "measurement did not start")
        # No bridge polling, captures or streaming during the device-clock window.
        time.sleep(12.3 if profile else 32.3)
        wait_for(lambda: value(application, "measuring") == "false", "measurement did not finish")
        raw = agent.metrics()
        (run / f"concept-{repetition}-raw.json").write_text(json.dumps(raw, indent=2) + "\n")
        result = validate(raw, agent.expected_sha256, effect, preset, profile)
        results.append(result)
        append_event(run, {"phase": "concept", "repetition": repetition, **result})
    (run / "concept-results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if profile or all(r["qualified"] for r in results) else 1

def interactive(application, agent, run, effect, preset):
    select(application, effect, preset)
    print("Commands: select EFFECT, preset default|reduced, pause, resume, step, restart, capture, quit", flush=True)
    capture_number = 0
    while True:
        try:
            parts = input("concept> ").split()
        except EOFError:
            return 0
        if not parts:
            continue
        command, *args = parts
        if command == "quit":
            return 0
        if command == "select" and len(args) == 1:
            effect = args[0]
            select(application, effect, preset)
        elif command == "preset" and len(args) == 1:
            preset = args[0]
            select(application, effect, preset)
        elif command in {"pause", "resume", "step", "restart"} and not args:
            action(application, command)
        elif command == "capture" and not args:
            action(application, "pause")
            wait_for(lambda: value(application, "paused") == "true", "pause was not acknowledged")
            capture_number += 1
            capture(agent, run / f"concept-{capture_number}.png")
        else:
            print("Unknown command or arguments", flush=True)

def run_concept(arguments, run: Path):
    from .cli import connect_agent, ensure_application, CHECK_AGENT_CAPABILITIES
    effect = arguments.effect if arguments.command == "concept" else arguments.concept
    if effect not in EFFECTS or arguments.app != "mini-magik":
        raise ValueError("select one supported concept with --app mini-magik")
    profile = bool(getattr(arguments, "profile", False))
    profile_id = f"{run.name}-{uuid.uuid4().hex[:8]}" if profile else None
    agent, status = connect_agent(run, CHECK_AGENT_CAPABILITIES | {"mini-display-plan-v1", "capture-framebuffer", "device-control-v1", "artifacts-v1"})
    # Main's confirmed mode must be checked before taking display ownership.
    display = agent._successful("display-status").get("reply", "")
    if "active=hdmi-" not in display or "pending=none" not in display:
        raise ValueError("concept qualification requires a confirmed HDMI mode")
    ensure_application(agent, status, run, "mini-magik")
    try:
        with fresh_session(agent, profile_id=profile_id) as application:
            if arguments.command == "concept":
                return interactive(application, agent, run, effect, arguments.preset)
            result = measure(application, agent, run, effect, arguments.preset, profile)
            capture(agent, run / "concept-final.png")
        if profile:
            for name in ("profile.json", "profile.folded", "flamegraph.svg"):
                (run / name).write_bytes(agent.read_profile_artifact(profile_id, name))
        return result
    finally:
        # The test bridge restores a persistent Mini; explicitly return ownership to Main.
        agent.stop()
        append_event(run, {"phase": "concept-cleanup", "main_restored": True})
