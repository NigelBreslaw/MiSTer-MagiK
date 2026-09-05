from types import SimpleNamespace

from magik2.testing import one_element


def test_one_element_filters_the_accessibility_role_query_by_label() -> None:
    target = SimpleNamespace(accessible_label="build-label")

    class Query:
        def match_accessible_role(self, role):
            assert role.name == "Text"
            return self

        def find_all(self):
            return [target]

    root = SimpleNamespace(query_descendants=lambda: Query())
    application = SimpleNamespace(first_window=SimpleNamespace(root_element=root))

    assert one_element(application, "build-label") is target
