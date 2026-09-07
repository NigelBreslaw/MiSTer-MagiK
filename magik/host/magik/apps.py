"""Two ordinary consumers of the same build, delivery and observation workflow."""

from dataclasses import dataclass
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
    "mini-magik": Application("mini-magik", "magik/probe", "mini-magik"),
    "magik": Application(
        "magik",
        "apps/mister",
        "mister-magik-fb",
        "release-device-ui-tests",
        ("tooling",),
        frozenset({"main-managed-magik"}),
    ),
}


def application(name: str = "mini-magik") -> Application:
    return APPLICATIONS[name]


def repository() -> Path:
    return Path(__file__).resolve().parents[3]
