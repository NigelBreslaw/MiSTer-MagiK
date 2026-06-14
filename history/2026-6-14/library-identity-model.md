# Library identity model

MiSTer MagiK now stores a normalized identity layer beside the older launcher
catalog tables. The old `games`, `launch_plans`, and `launcher_catalog` tables
remain the runtime surface while the identity catalog is introduced.

## Terms

- **Launchable:** one installed thing the UI can start. Examples: an arcade MRA,
  a Neo Geo MGL, or a virtual launch generated from an archive entry.
- **Identity:** the canonical machine/game key from an outside source. For
  arcade and Neo Geo v1, this is the MAME setname.
- **Family:** a parent/clone group. If MAME says `1942b` has parent `1942`, both
  launchables belong to family `1942`.
- **Projection:** a materialized UI view derived from launchables, identities,
  metadata, and asset links. Preferred-only and variant projections will be added
  after the identity data is in place.

## Current rules

Arcade MRAs link to MAME identity by their `<setname>`.

Neo Geo MGLs link to MAME identity by the parenthesized setname in the final
payload or launcher name, such as `Metal Slug 3 (mslug3).mgl`.

`/media/fat/mister-magik/mame.sqlite3` is loaded opportunistically. When it is
present, identity rows are enriched with parent family, title, year, and
manufacturer. When it is absent or missing a setname, the launchable remains in
the database and gets an unenriched setname identity so the UI can still launch
the game.

This avoids going back to filename/title heuristics for screenshots. The next
steps are preferred/variant UI projections and formal asset-pack indexing.
