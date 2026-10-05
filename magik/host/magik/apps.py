"""Two ordinary consumers of the same build, delivery and observation workflow."""

from dataclasses import dataclass, replace
import os
from pathlib import Path


@dataclass(frozen=True)
class Application:
    name: str
    package: str
    binary: str
    profile: str = "release"
    features: tuple[str, ...] = ()
    agent_capabilities: frozenset[str] = frozenset()


APPLICATIONS = {
    "mini-magik": Application(
        "mini-magik",
        "magik/probe",
        "mini-magik",
        agent_capabilities=frozenset({"mini-display-plan-v1"}),
    ),
    "magik": Application(
        "magik",
        "apps/mister",
        "mister-magik-fb",
        "release-device-ui-tests",
        ("tooling",),
        frozenset({"main-managed-magik", "card-artwork-v1"}),
    ),
}


def application(name: str = "mini-magik") -> Application:
    app = APPLICATIONS[name]
    if name == "mini-magik" and os.environ.get("MAGIK_MINI_PRODUCTION_BUILD") == "1":
        return replace(app, profile="release-device")
    if name == "magik":
        sampler = os.environ.get("MAGIK_CARD_SAMPLER_AB", "current")
        if sampler not in {"current", "axis"}:
            raise ValueError("MAGIK_CARD_SAMPLER_AB must be current or axis")
        if sampler == "axis":
            return replace(app, features=(*app.features, "card-axis-filter"))
    return app


def repository() -> Path:
    return Path(__file__).resolve().parents[3]
