# MAME Metadata Database

MiSTer MagiK uses MAME as the v1 source of truth for arcade and Neo Geo machine
identity. The host tool can generate a compact SQLite database from a pinned
MAME `-listxml` export:

```bash
scripts/mister mame-metadata-build \
  --mame /Users/nigelb/Downloads/mame0288-arm64/mame \
  --out build/mame.sqlite3
```

The initial pin is MAME 0.288. The generated `mame_machines` table stores only
the fields MagiK needs for browsing, filtering, and family linking: setname,
parent setname, title, year, manufacturer, source file, display orientation and
size, player/control/button counts, driver status, emulation status, savestate,
and source version.

This database is intended to be shipped beside the frontend as metadata, not
compiled into the binary. The SD-card library database should link discovered
launchables to `mame_machines.setname` rather than infer clone families from
filenames or screenshot names.
