import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "scope", Path(__file__).resolve().parents[2] / "scripts/check_tooling_scope.py"
)
scope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scope)


def test_consumers_remain_independent_and_label_cannot_hide_mixed_core_work():
    assert scope.scope_error(["magik2/scenarios/test_probe.py"], False) is None
    assert scope.scope_error(["scripts/magik2"], False)
    assert scope.scope_error(
        ["magik2/host/magik2/client.py", "apps/mister/ui.slint"], True
    )
    assert (
        scope.scope_error(
            ["magik2/host/magik2/client.py", "magik2/scenarios/test_probe.py"], True
        )
        is None
    )


def test_only_initial_introduction_allows_the_exact_runtime_integration():
    paths = ["magik2/AGENTS.md", *scope.INTRODUCTION_PATHS]
    assert scope.scope_error(paths, True)
    assert scope.scope_error(paths, True, introduction=True) is None
    assert scope.scope_error(paths, False, introduction=True)
    assert scope.scope_error(
        paths + ["apps/mister/src/main.rs"], True, introduction=True
    )
