"""One explicitly selected concept using the existing native Slint test session."""

from __future__ import annotations

import json
import math
import time
import uuid
from weakref import WeakKeyDictionary
from pathlib import Path
from .testing import fresh_session, one_element
from .results import append_event
from .capture import capture_png

EFFECTS = (
    "wireframe-terrain",
    "raster-waves",
    "texture-tunnel",
    "palette-aurora",
    "starfield-comets",
    "pixel-dissolve",
    "mirror-floor",
    "light-sweep",
    "depth-parallax",
    "point-cloud-morph",
    "diagnostic",
)
PRESETS = ("default", "reduced")


_elements = WeakKeyDictionary()


def element(application, name):
    cache = _elements.setdefault(application, {})
    if name not in cache:
        cache[name] = one_element(application, "concept-" + name)
    return cache[name]


def value(application, name):
    return element(application, name).accessible_value


def wait_for(predicate, message, timeout=15):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise RuntimeError(message)
        time.sleep(0.025)


def action(application, name):
    element(application, name).invoke_accessible_default_action()


def select(application, effect, preset):
    if effect not in EFFECTS or preset not in PRESETS:
        raise ValueError("unsupported concept or preset")
    previous = value(application, "generation")
    action(application, f"select-{effect}-{preset}")
    wait_for(
        lambda: value(application, "generation") != previous,
        "concept preparation timed out",
        timeout=30,
    )
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
    if (
        w.get("instrumented") is not profile
        or not duration <= w.get("elapsed_ms", 0) <= duration + 1000
    ):
        raise ValueError("invalid concept measurement boundaries")
    if w.get("end_ms", 0) - w.get("start_ms", 0) != w["elapsed_ms"]:
        raise ValueError("inconsistent concept clock")
    context = w.get("context", {})
    if (context.get("concept"), context.get("preset"), context.get("route")) != (
        effect,
        preset,
        "hdmi",
    ):
        raise ValueError("concept configuration mismatch")
    if not all(type(w.get(key)) is int and w[key] > 0 for key in ("width", "height")):
        raise ValueError("missing resolved buffer geometry")
    cpu = w.get("process_cpu_percent")
    rss = w.get("peak_rss_bytes")
    refresh = w.get("refresh_hz")
    if not all(
        type(x) in (int, float) and math.isfinite(x) and x > 0
        for x in (cpu, rss, refresh)
    ):
        raise ValueError("CPU, RSS or refresh evidence unavailable")
    if w.get("evidence_error") or not w.get("drop_baseline_available"):
        raise ValueError("invalid physical evidence")
    n = w.get("presentations", 0)
    cadence = n > 0 and all(
        w.get(k) == n
        for k in (
            "physical_latch_posts",
            "physical_latch_flips",
            "presented_vblanks",
            "owned_vblanks",
        )
    )
    clean = all(
        w.get(k) == 0 for k in ("physical_drops", "latch_drops", "latch_rejections")
    )
    passed = (
        cadence
        and clean
        and 59 <= refresh <= 61
        and cpu < 150
        and rss <= 128 * 1024 * 1024
    )
    return {
        **w,
        "sha256": sha256,
        "fps": n * 1000 / w["elapsed_ms"],
        "qualified": passed and not profile,
        "instrumented": profile,
    }


def measure(application, agent, run, effect, preset, profile):
    results = []
    for repetition in range(1 if profile else 2):
        select(application, effect, preset)
        action(application, "measure")
        wait_for(
            lambda: value(application, "measuring") == "true",
            "measurement did not start",
        )
        # No bridge polling, captures or streaming during the device-clock window.
        time.sleep(12.3 if profile else 32.3)
        wait_for(
            lambda: value(application, "measuring") == "false",
            "measurement did not finish",
        )
        raw = agent.metrics()
        (run / f"concept-{repetition}-raw.json").write_text(
            json.dumps(raw, indent=2) + "\n"
        )
        result = validate(raw, agent.expected_sha256, effect, preset, profile)
        results.append(result)
        append_event(run, {"phase": "concept", "repetition": repetition, **result})
    (run / "concept-results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if profile or all(r["qualified"] for r in results) else 1


# Device timeline bookmarks, in milliseconds. Captures happen after measurement.
BOOKMARKS = {
    "point-cloud-morph": (5000, 12500, 20000),
    "depth-parallax": (320, 16000),
    "light-sweep": (1500, 3000),
    "mirror-floor": (2048, 4096),
    "pixel-dissolve": (1300, 3200),
    "starfield-comets": (4096, 8192),
    "palette-aurora": (6144, 12288),
    "texture-tunnel": (4000, 8000),
    "raster-waves": (1024, 2048),
    "wireframe-terrain": (64000, 128000),
    "diagnostic": (100, 7680),
}


def review(application, agent, run, effect, preset):
    # Exercise switching and the other preset outside measured windows.
    select(application, "diagnostic", "reduced")
    select(application, effect, "reduced" if preset == "default" else "default")
    action(application, "pause")
    wait_for(lambda: value(application, "paused") == "true", "pause failed")
    before = int(value(application, "frame"))
    time.sleep(0.1)
    if int(value(application, "frame")) != before:
        raise RuntimeError("paused concept advanced")
    action(application, "step")
    wait_for(lambda: int(value(application, "frame")) != before, "step failed")
    if int(value(application, "frame")) - before not in (16, 17):
        raise RuntimeError("step must advance exactly one nominal interval")
    action(application, "restart")
    wait_for(lambda: value(application, "frame") == "0", "restart failed")
    select(application, effect, preset)
    bookmarks = [("initial", 0)]
    if effect == "point-cloud-morph":
        bookmarks.append(("cabinet", 5000))
    bookmarks += [
        ("midpoint", BOOKMARKS[effect][-2]),
        ("boundary", BOOKMARKS[effect][-1]),
    ]
    for label, target in bookmarks:
        action(application, "capture-" + label)
        wait_for(
            lambda target=target: (
                value(application, "paused") == "true"
                and target <= int(value(application, "frame")) <= target + 17
            ),
            "capture bookmark timed out",
            timeout=180,
        )
        elapsed = int(value(application, "frame"))
        capture(agent, run / f"concept-{label}.png")
        append_event(
            run,
            {"phase": "concept-capture", "target_ms": target, "elapsed_ms": elapsed},
        )
    action(application, "resume")
    append_event(run, {"phase": "concept-controls", "passed": True})


def interactive(application, agent, run, effect, preset):
    select(application, effect, preset)
    print(
        "Commands: select EFFECT, preset default|reduced, pause, resume, step, restart, capture, quit",
        flush=True,
    )
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
            wait_for(
                lambda: value(application, "paused") == "true",
                "pause was not acknowledged",
            )
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
    agent, status = connect_agent(
        run,
        CHECK_AGENT_CAPABILITIES
        | {
            "mini-display-plan-v1",
            "mini-concepts-v3",
            "capture-framebuffer",
            "device-control-v1",
            "artifacts-v1",
        },
    )
    # Main's confirmed mode must be checked before taking display ownership.
    display = agent.device_operation("display-status").get("reply", "")
    if "active=hdmi-" not in display or "pending=none" not in display:
        raise ValueError("concept qualification requires a confirmed HDMI mode")
    ensure_application(agent, status, run, "mini-magik")
    try:
        with fresh_session(
            agent, profile_id=profile_id, concept_session=True
        ) as application:
            if arguments.command == "concept":
                return interactive(application, agent, run, effect, arguments.preset)
            result = measure(application, agent, run, effect, arguments.preset, profile)
            if not profile:
                review(application, agent, run, effect, arguments.preset)
        if profile_id is not None:
            for name in ("profile.json", "profile.folded", "flamegraph.svg"):
                (run / name).write_bytes(agent.read_profile_artifact(profile_id, name))
            metadata = json.loads((run / "profile.json").read_text())
            if (
                metadata.get("run_id") != profile_id
                or metadata.get("sha256") != agent.expected_sha256
                or metadata.get("complete") is not True
            ):
                raise ValueError("profile identity or completion mismatch")
        return result
    except Exception as error:
        append_event(run, {"phase": "concept-error", "error": str(error)})
        raise
    finally:
        # The native session restarts Mini with its last selected concept and
        # preset. Leave the active work on HDMI, as requested for iteration.
        append_event(run, {"phase": "concept-cleanup", "retained_concept": effect})
