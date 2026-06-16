# Metadata and asset deploy integration

`scripts/deploy-rust.sh` now accepts optional metadata/asset flags:

```bash
scripts/deploy-rust.sh --mame-metadata
scripts/deploy-rust.sh --hbmame-metadata
scripts/deploy-rust.sh --asset-packs
scripts/deploy-rust.sh --mame-metadata --hbmame-metadata --asset-packs
```

`--mame-metadata` deploys:

```text
build/mame.sqlite3 -> /media/fat/mister-magik/mame.sqlite3
```

If `build/mame.sqlite3` is missing, deploy builds it with:

```bash
scripts/mister mame-metadata-build --out build/mame.sqlite3
```

`--hbmame-metadata` deploys:

```text
build/hbmame.sqlite3 -> /media/fat/mister-magik/hbmame.sqlite3
```

If `build/hbmame.sqlite3` is missing and `MISTER_HBMAME_BIN` is set, deploy
builds it from HBMAME `-listxml` with:

```bash
scripts/mister mame-metadata-build --out build/hbmame.sqlite3 --mame "$MISTER_HBMAME_BIN"
```

If neither a local DB nor `MISTER_HBMAME_BIN` is available, deploy asks the
device to create a supplemental `hbmame.sqlite3` from parsed MRA parent rows in
the current library database. When the device library database is missing, deploy
refreshes once, creates the supplemental DB, then refreshes again so the new
metadata participates in clone-family projection.

`--asset-packs` deploys:

```text
build/neogeo-screenshots/neogeo-screenshots.mmlz4b
  -> /media/fat/mister-magik/assets/neogeo-screenshots.mmlz4b
```

Build that pack first with:

```bash
scripts/build-neogeo-screenshot-pack.sh
```

When metadata or asset flags are used, deploy refreshes the device library
database after the files are installed. The library DB fingerprints
`mame.sqlite3` and `hbmame.sqlite3`, so metadata changes force a catalog rebuild
instead of using a stale cached projection.

Runtime preview loading now supports multiple v1 screenshot packs. It checks:

- `MISTER_PREVIEW_ARCHIVES`, colon-separated, for explicit override packs.
- `MISTER_PREVIEW_ARCHIVE`, the existing single-pack override.
- The existing auto-detected arcade raw/lz4 pack.
- `/media/fat/mister-magik/assets/neogeo-screenshots.mmlz4b` when present, or
  `MISTER_NEOGEO_PREVIEW_ARCHIVE` when set.

Diagnostics:

```bash
scripts/mister-asset-diagnostics.sh
scripts/mister-asset-diagnostics.sh 1942
```

The diagnostics query family membership, missing preferred screenshots by
system, asset link reasons, and smoke rows for `1941`, `1942`, and `mslug3`.
