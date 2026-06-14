# Arcade Preview Identity Regression

The arcade screenshot regression appeared after the launcher stopped relying on
the scraper-generated `gamelist.xml` metadata path. Those XML entries had already
resolved clone/variant rows to a representative screenshot, for example all
`1941: Counter Attack` variants pointed at `1941u.png`.

The newer SQLite scanner attached previews by exact MRA `<setname>` against the
raw preview archive stems. That made rows such as `1941j`, `1941`, and `1941r1`
miss the available `1941u` screenshot. A later stale-cache bug made this worse:
an old populated `library.sqlite3` could be treated as ready even after a preview
archive appeared or changed, leaving materialized `has_image=0` rows in place.

Filename suffix stripping and title-family guessing were rejected as the long
term fix. MiSTer MagiK should derive arcade identity from MAME/MESS metadata:
MRA setname -> MAME machine -> parent/clone family -> screenshot asset. The
short-term safe fix is only to refresh a populated catalog when the preview
archive fingerprint is stale.
