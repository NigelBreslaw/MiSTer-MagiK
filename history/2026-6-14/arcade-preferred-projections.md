# Arcade preferred projections

The library database now materializes two UI projection tables for arcade-style
systems:

- `ui_arcade_preferred`: one row per identity family for the default arcade and
  Neo Geo list.
- `ui_arcade_variants`: every installed launchable in those families, with a
  `preferred` flag.

The current launcher catalog remains as a compatibility mirror. Arcade and Neo
Geo rows come from `ui_arcade_preferred`; other systems are appended through the
existing launcher path until they get their own projections.

Preferred selection is intentionally simple:

1. If the installed MAME parent exists, use it.
2. Otherwise choose a deterministic installed child, preferring rows with a
   screenshot and then stable title/path ordering.

Debug fields are present in the projection tables: `identity_id`, `family_id`,
`parent_setname`, and `preferred_reason`. A future details/variants view should
read `ui_arcade_variants` rather than rediscovering clone relationships.
