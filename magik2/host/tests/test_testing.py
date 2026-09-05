from types import SimpleNamespace

from magik2.testing import one_element


def test_one_element_filters_the_visual_tree_query_by_accessibility_label() -> None:
    target = SimpleNamespace(accessible_label="build-label")

    class Query:
        def match_inherits(self, type_name: str):
            assert type_name == "Text"
            return self

        def find_all(self):
            return [target]

    root = SimpleNamespace(query_descendants=lambda: Query())
    application = SimpleNamespace(first_window=SimpleNamespace(root_element=root))

    assert one_element(application, "build-label") is target
