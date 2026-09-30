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
    "raster-waves",
    "texture-tunnel",
    "starfield",
    "pixel-dissolve",
    "light-sweep",
    "diagnostic",
    "launcher-cards",
    "arcade-transition",
)
PRESETS = (
    "default",
    "reduced",
    "dithered",
    "rgb888",
    "cached",
    "cached-fast",
    "scanline",
)
RENDER_LABS = ("launcher-cards", "arcade-transition")


def supported(effect, preset):
    if effect == "arcade-transition" and preset in (
        "cached",
        "cached-fast",
        "scanline",
    ):
        return True
    return effect in EFFECTS and preset in (
        ("default", "dithered", "rgb888")
        if effect in RENDER_LABS
        else ("default", "reduced")
    )


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
    if not supported(effect, preset):
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
    if effect in RENDER_LABS and (w["width"], w["height"]) != (960, 540):
        raise ValueError("rendering lab geometry mismatch")
    fps = n * 1000 / w["elapsed_ms"]
    if effect in RENDER_LABS:
        passed = passed and abs(fps - refresh) <= 0.1
    phase_ms = context.get("animation_elapsed_ms")
    motion_started = metrics.get("motion_started_ms")
    motion_qualified = effect not in RENDER_LABS or (
        context.get("animation_clock") == "monotonic"
        and type(phase_ms) is int
        and type(motion_started) is int
        and abs(phase_ms - (w["end_ms"] - motion_started)) <= 100
    )
    build_qualified = (
        effect not in RENDER_LABS or context.get("build_profile") == "release-device"
    )
    return {
        **w,
        "sha256": sha256,
        "fps": fps,
        "qualified": passed and build_qualified and motion_qualified and not profile,
        "motion_qualified": motion_qualified,
        "build_qualified": build_qualified,
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
        print(
            f"{effect}/{preset}: repetition={repetition + 1} "
            f"fps={result['fps']:.3f} cpu={result['process_cpu_percent']:.1f}% "
            f"drops={result['physical_drops']} render_p99_us={result.get('render_p99_us')} "
            f"qualified={result['qualified']}",
            flush=True,
        )
        append_event(run, {"phase": "concept", "repetition": repetition, **result})
    (run / "concept-results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if profile or all(r["qualified"] for r in results) else 1


# Device timeline bookmarks, in milliseconds. Captures happen after measurement.
BOOKMARKS = {
    "launcher-cards": (210, 420),
    "arcade-transition": (500, 1000),
    "light-sweep": (1500, 3000),
    "pixel-dissolve": (1300, 3200),
    "starfield": (4096, 8192),
    "texture-tunnel": (4000, 8000),
    "raster-waves": (1024, 2048),
    "diagnostic": (100, 7680),
}


def review(application, agent, run, effect, preset):
    # Exercise switching and the other preset outside measured windows.
    select(application, "diagnostic", "reduced")
    select(
        application,
        effect,
        ("dithered" if effect in RENDER_LABS else "reduced")
        if preset == "default"
        else "default",
    )
    action(application, "restart")
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
    bookmarks = [
        ("initial", 0),
        ("midpoint", BOOKMARKS[effect][0]),
        ("boundary", BOOKMARKS[effect][1]),
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
        "Commands: select EFFECT, preset default|reduced|dithered|rgb888|cached|cached-fast|scanline, pause, resume, step, restart, capture, quit",
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


def verify_installed(fields, sha256):
    if len(sha256) != 64 or any(c not in "0123456789abcdef" for c in sha256):
        raise ValueError(
            "installed SHA-256 must be 64 lowercase hexadecimal characters"
        )
    if not (
        fields.get("running")
        and fields.get("ready")
        and fields.get("artifact") == "mini-magik"
        and fields.get("running_sha256") == sha256
    ):
        raise ValueError(
            "running development artifact does not match requested SHA-256"
        )


def profile_preparation(application, agent, run, effect, preset, *, sampled=True):
    action(application, "profile-preparation" if sampled else "bench-preparation")
    select(application, effect, preset)
    generation = int(value(application, "generation"))
    samples = []

    def completed():
        raw = agent.metrics()
        context = raw.get("context") or {}
        preparation_complete = (
            (context.get("preparation_profile") or {}).get("complete")
            if sampled
            else context.get("preparation_benchmark")
        )
        if (
            preparation_complete
            and context.get("startup")
            and context.get("concept_generation") == generation
        ):
            samples.append(raw)
            return True
        return False

    wait_for(completed, "preparation profile was not published", timeout=30)
    raw = samples[0]
    context = raw["context"]
    if raw.get("sha256") != agent.expected_sha256 or (
        context.get("concept"),
        context.get("preset"),
        context.get("build_profile"),
    ) != (effect, preset, "release-device"):
        raise ValueError("preparation profile identity or build mismatch")
    if (
        raw.get("evidence_error", "missing") is not None
        or raw.get("latch_rejections") != 0
        or any(
            type(raw.get(key)) is not int or raw[key] < 1
            for key in ("presentations", "physical_latch_posts", "physical_latch_flips")
        )
    ):
        raise ValueError("startup measurement lacks valid first-presentation evidence")
    (run / "preparation-raw.json").write_text(json.dumps(raw, indent=2) + "\n")
    append_event(
        run,
        {
            "phase": "preparation-profile" if sampled else "preparation-benchmark",
            "context": context,
        },
    )
    if not sampled:
        capture(agent, run / "startup.png")
    print(
        f"{effect}/{preset}: cold preparation={context['preparation_ms']}ms "
        f"first confirmed present={context['startup']['preparation_to_first_confirmed_present_us']}us "
        f"({'instrumented' if sampled else 'uninstrumented'})",
        flush=True,
    )
    return 0


def run_concept(arguments, run: Path):
    from .cli import connect_agent, ensure_application, CHECK_AGENT_CAPABILITIES

    effect = arguments.effect if arguments.command == "concept" else arguments.concept
    if not supported(effect, arguments.preset) or arguments.app != "mini-magik":
        raise ValueError("select one supported concept with --app mini-magik")
    preparation_profile = bool(getattr(arguments, "profile_preparation", False))
    preparation = preparation_profile or bool(
        getattr(arguments, "bench_preparation", False)
    )
    profile = bool(getattr(arguments, "profile", False)) or preparation_profile
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
            "lifecycle-v1",
        },
    )
    # Main's confirmed mode must be checked before taking display ownership.
    display = agent.device_operation("display-status").get("reply", "")
    if "active=hdmi-" not in display or "pending=none" not in display:
        raise ValueError("concept qualification requires a confirmed HDMI mode")
    installed = getattr(arguments, "installed_sha256", None)
    if installed:
        verify_installed(status.fields, installed)
        agent.artifact = "mini-magik"
        agent.expected_sha256 = installed
        append_event(run, {"phase": "artifact", "sha256": installed, "installed": True})
    else:
        ensure_application(agent, status, run, "mini-magik")
    try:
        with fresh_session(
            agent, profile_id=profile_id, concept_session=True
        ) as application:
            if arguments.command == "concept":
                return interactive(application, agent, run, effect, arguments.preset)
            result = (
                profile_preparation(
                    application,
                    agent,
                    run,
                    effect,
                    arguments.preset,
                    sampled=preparation_profile,
                )
                if preparation
                else measure(application, agent, run, effect, arguments.preset, profile)
            )
            if not profile and not preparation:
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
        if preparation:
            # End a startup-only experiment when its first frame is complete.
            # Avoid leaving a restored Mini storyboard running after this command.
            stopped = agent.stop()
            append_event(run, {"phase": "preparation-stop", "result": stopped})
