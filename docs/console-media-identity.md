# Console Media Identity

MiSTer MagiK uses MAME software lists as the canonical identity source for
console media where practical. Runtime must not depend on scraper screenshot
filenames, `gamelist.xml`, or scanning screenshot folders.

The XML boundary is intentionally narrow: MAME `-listxml` and MAME `hash/*.xml`
software lists are offline inputs to `scripts/mister mame-metadata-build`. The
scanner and launcher read the generated SQLite metadata, not those XML files.

## Target Systems

The first console identity pass covers:

- NES: MAME list `nes`
- SNES: MAME list `snes`
- Nintendo 64: MAME list `n64`
- Sega Master System: MAME list `sms`
- Mega Drive: MAME list `megadriv`
- Saturn: MAME list `saturn`

Library identities use:

- namespace: `mame-software`
- identity id: `<list_name>:<software_name>`
- family id: parent software item when MAME has one, otherwise the same item

## Metadata Build

`scripts/mister mame-metadata-build` writes arcade machine metadata and console
software-list metadata into the same `mame.sqlite3`. The command can read MAME
machine XML from `mame -listxml` or `--listxml`; it reads console software-list
XML only from explicit `--software-list` paths or the target files inside a
`--software-dir` MAME hash directory.

Important tables:

- `mame_machines`
- `mame_software_items`
- `mame_software_hashes`

Build with a local MAME hash directory:

```bash
scripts/mister mame-metadata-build \
  --mame mame \
  --software-dir /path/to/mame/hash \
  --out build/mame.sqlite3
```

When reusing an existing machine-only DB and adding software lists:

```bash
scripts/mister mame-metadata-build \
  --machine-sqlite build/mame.sqlite3 \
  --software-dir build/mame-hash \
  --out build/mame.sqlite3
```

Distribution CI resolves the MAME `hash/` directory from the installed MAME
package, passes `--software-dir`, and verifies all target lists have nonzero
rows before packaging.

## Matching Rules

Cartridge systems are matched by normalized ROM bytes:

- NES: strip a common 16-byte iNES/NES2 header before matching, then try raw.
- SNES: try 512-byte copier-header-stripped bytes, then raw.
- Nintendo 64: try raw, byte-swapped, word-swapped, and reversed word forms.
- SMS and Mega Drive: try raw bytes.

Saturn matching prefers CHD raw SHA1 from the CHD header against software-list
`disk_sha1`. Saturn disc header parsing records product/region metadata, but v1
does not alter Main_MiSTer launch behavior or BIOS selection.

Filename/title fallback is only a fallback into MAME software-list titles. It is
not a screenshot naming contract.

## Screenshot Packs

Console screenshot packs live under:

```text
/media/fat/mister-magik/assets/nes-screenshots.mmlz4b
/media/fat/mister-magik/assets/snes-screenshots.mmlz4b
/media/fat/mister-magik/assets/n64-screenshots.mmlz4b
/media/fat/mister-magik/assets/sms-screenshots.mmlz4b
/media/fat/mister-magik/assets/megadrive-screenshots.mmlz4b
/media/fat/mister-magik/assets/saturn-screenshots.mmlz4b
```

Archive entries use filesystem-safe canonical keys:

```text
mame-software__megadriv__sonic.rgb565
```

Build a pack from MagiK-owned source images:

```bash
scripts/build-console-screenshot-pack.sh \
  --system megadrive \
  --input build/source-screenshots/megadrive \
  --deploy
```

Input image stems must already be canonical, or they must be the MAME software
short name for the chosen system. For example, `sonic.png` under
`--system megadrive` becomes `mame-software__megadriv__sonic`.

The builder deliberately rejects scraper/title stems such as
`Sonic The Hedgehog (USA).png`. If `build/mame.sqlite3` exists, it also verifies
each software name against `mame_software_items`. Convert scraper output into a
staging directory with canonical names before building the pack.

Stage scraper/title screenshots offline with the Rust host tool:

```bash
scripts/mister console-screenshot-stage \
  --system saturn \
  --input build/source-screenshots/saturn-scraper \
  --output build/source-screenshots/saturn-canonical \
  --report build/source-screenshots/saturn-stage-report.tsv
```

The report records `mapped`, `unmatched`, `ambiguous`, and `collision` rows.
Resolve ambiguous rows with an overrides TSV of `source_stem<TAB>software_name`;
the runtime still only sees canonical pack entries.

## Runtime Projection

The launcher catalog marks `has_preview=1` and stores the resolved
`preview_archive_path` plus `preview_asset_key` when it can resolve a current
asset entry by:

1. exact software item,
2. parent software item,
3. sibling in the same software family.

Preview asset changes are folded into the catalog stamp. When a pack is added
or removed, the next validation treats the catalog as stale and runs the normal
database builder, which recomputes console preview fields from the current asset
packs.

## Performance Notes

SQLite does not load the full `library.sqlite3` into RAM at startup. The slow
operation is building the database, especially the first identity pass over
console ROM payloads on MiSTer's exFAT/FUSE storage.

Avoid doing screenshot-pack construction on the MiSTer hot path. Longer term,
prefer host-built identity/cache data or persistent per-file hashes so a first
device build does not need to reread large console libraries.
