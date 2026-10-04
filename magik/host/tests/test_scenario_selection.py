import ast
from pathlib import Path
from types import SimpleNamespace

from magik.cli import CHECK_SCENARIOS
from magik.scenario_runner import pytest_collection_modifyitems


def test_profile_selects_only_profile_and_default_keeps_shared_workload():
    class Item:
        path = SimpleNamespace(name="test_probe.py")

        def __init__(self, profile):
            self.profile = profile

        def get_closest_marker(self, _):
            return object() if self.profile else None

    ordinary = [Item(False), Item(False), Item(False)]
    profiled = Item(True)
    for profile, expected in [(False, ordinary), (True, [profiled])]:
        config = SimpleNamespace(
            getoption=lambda key, profile=profile: (
                "mini-magik" if key == "--magik-app" else profile
            ),
            hook=SimpleNamespace(pytest_deselected=lambda **_: None),
        )
        items = [*ordinary, profiled]
        pytest_collection_modifyitems(config, items)
        assert items == expected


def test_every_advertised_magik_check_scenario_selects_a_test():
    scenarios = Path(__file__).resolve().parents[2] / "scenarios/test_magik.py"
    tree = ast.parse(scenarios.read_text())
    tests = {
        node.name
        for node in tree.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        and node.name.startswith("test_")
    }
    for scenario in CHECK_SCENARIOS:
        selector = scenario.replace("-", "_")
        assert any(selector in name for name in tests), scenario


def test_full_app_routes_forward_requested_frame_evidence(
    monkeypatch, tmp_path, pytestconfig
):
    import importlib.util
    from unittest.mock import Mock
    from magik import animation_benchmark

    pytestconfig.addinivalue_line("markers", "magik_profile: sampled device diagnostic")
    path = Path(__file__).resolve().parents[2] / "scenarios/test_magik.py"
    monkeypatch.syspath_prepend(str(path.parent))
    spec = importlib.util.spec_from_file_location("launcher_scenarios", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    benchmark = Mock(return_value={})
    monkeypatch.setattr(animation_benchmark, "animation_roundtrip", benchmark)
    monkeypatch.setattr(module, "append_event", Mock())
    request = SimpleNamespace(config=SimpleNamespace(getoption=lambda _: "phases"))
    app, agent = object(), object()
    module.test_animation_app((app, agent, tmp_path, None), 2, "arcade", request)
    benchmark.assert_called_once_with(
        app, agent, tmp_path, 2, route="arcade", frame_evidence="phases"
    )
