# MAME Metadata Database

MiSTer MagiK uses MAME as the v1 source of truth for arcade and Neo Geo machine
identity. The host tool can generate a compact SQLite database from a pinned
MAME `-listxml` export:

```bash
scripts/mister mame-metadata-build \
  --mame /path/to/mame \
  --out build/mame.sqlite3
```

The builder also accepts a pre-generated XML export:

```bash
scripts/mister mame-metadata-build \
  --listxml build/mame-listxml.xml \
  --out build/mame.sqlite3
```

If neither `--mame` nor `--listxml` is provided, the tool uses `MAME_BIN` or a
`mame` executable on `PATH`. It deliberately has no developer-local fallback
path.

The initial pin is MAME 0.288. The generated `mame_machines` table stores only
the fields MagiK needs for browsing, filtering, and family linking: setname,
parent setname, title, year, manufacturer, source file, display orientation and
size, player/control/button counts, driver status, emulation status, savestate,
and source version.

This database is intended to be shipped beside the frontend as metadata, not
compiled into the binary. The SD-card library database should link discovered
launchables to `mame_machines.setname` rather than infer clone families from
filenames or screenshot names.

## HBMAME supplemental metadata

MiSTer alternative arcade MRAs can reference HBMAME setnames that are absent
from normal MAME metadata. MagiK supports an optional sibling database:

```text
build/hbmame.sqlite3 -> /media/fat/mister-magik/hbmame.sqlite3
```

It uses the same `mame_machines` schema and is checked after `mame.sqlite3`.
When the HBMAME database contains a setname, `launchable_identities.source` is
recorded as `hbmame`; otherwise MagiK falls back to the raw setname.

For release builds, the distribution workflow runs HBMAME on a Windows runner
to generate `-listxml`, then converts that XML to `build/hbmame.sqlite3` on the
Linux packaging runner. Configure either the workflow input
`hbmame_download_url` or the repository variable `HBMAME_DOWNLOAD_URL` with the
official HBMAME command-line archive URL. Bump `hbmame_cache_key` or repository
variable `HBMAME_CACHE_KEY` when changing the HBMAME archive/version.
