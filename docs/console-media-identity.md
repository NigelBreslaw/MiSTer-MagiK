# Console Media Identity

MiSTer MagiK uses MAME software lists as the canonical identity source for
console media where practical. Runtime must not depend on scraper screenshot
filenames, `gamelist.xml`, or scanning screenshot folders.

The XML boundary is intentionally narrow: MAME `-listxml` and MAME `hash/*.xml`
software lists are offline inputs to `mister mame-metadata-build`. The
scanner and launcher read the generated SQLite metadata, not those XML files.

## Target Systems

The first console identity pass covers:

- NES: MAME list `nes`
- SNES: MAME list `snes`
- Nintendo 64: MAME list `n64`
- Sega Master System: MAME list `sms`
- Mega Drive: MAME list `megadriv`
- Saturn: MAME list `saturn`
- Atari Lynx: MAME list `lynx`

Library identities use:

- namespace: `mame-software`
- identity id: `<list_name>:<software_name>`
- family id: parent software item when MAME has one, otherwise the same item

## Metadata Build

`mister mame-metadata-build` writes arcade machine metadata and console
software-list metadata into the same `mame.sqlite3`. The command can read MAME
machine XML from `mame -listxml` or `--listxml`; it reads console software-list
XML only from explicit `--software-list` paths or the target files inside a
`--software-dir` MAME hash directory. Arcade player counts and control types
come directly from each machine's `input` and `control` elements in `-listxml`.

Important tables:

- `mame_machines`
- `mame_software_items`
- `mame_software_hashes`

Build with a local MAME hash directory:

```bash
mister mame-metadata-build \
  --mame mame \
  --software-dir /path/to/mame/hash \
  --out build/mame.sqlite3
```

When reusing an existing machine-only DB and adding software lists:

```bash
mister mame-metadata-build \
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
- Atari Lynx: hash ROM content by default, trying a standard 64-byte
  `LYNX`-header-stripped payload before raw bytes. Lynx prefers the hash match
  and falls back to the normalized filename/title match; other cartridge
  systems retain the global hash opt-in policy.

Saturn matching prefers CHD raw SHA1 from the CHD header against software-list
`disk_sha1`. Saturn disc header parsing records product/region metadata, but v1
does not alter Main_MiSTer launch behavior or BIOS selection.

Filename/title fallback is only a fallback into MAME software-list titles. It is
not a screenshot naming contract.

## Screenshot Packs

Console screenshot packs live under:

```text
/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/neogeo-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/nes-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/snes-screenshots-256x224.mmlz4b
/media/fat/mister-magik/assets/n64-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/sms-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/megadrive-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/saturn-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/amiga-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/atarilynx-screenshots-160x102.mmlz4b
```

The default public pack size is `320x320`; SNES uses its native `256x224`
geometry, and Atari Lynx uses its native `160x102` landscape geometry, with
portrait titles stored as `102x160`.
Legacy fixed-name packs such as
`saturn-screenshots.mmlz4b` are still readable as fallback, but new runtime
downloads preserve the image size in the filename. The preview worker resolves
legacy catalog paths through
`/media/fat/mister-magik/assets/.screenshot-media-state.json` before falling
back to fixed-name files.

Archive entries use filesystem-safe canonical keys:

```text
mame-software__megadriv__sonic.rgb565
```

Build a pack from MagiK-owned source images in the private
`private/magik-cloud` submodule:

```bash
scripts/magik-cloud run -- scripts/build-console-screenshot-pack.sh \
  --system megadrive \
  --input data/sources/megadrive/canonical
```

Input image stems must already be canonical, or they must be the MAME software
short name for the chosen system. For example, `sonic.png` under
`--system megadrive` becomes `mame-software__megadriv__sonic`.

The builder deliberately rejects scraper/title stems such as
`Sonic The Hedgehog (USA).png`. If `data/mame.sqlite3` exists in
`magik-cloud`, it also verifies each software name against
`mame_software_items`. Convert scraper output into a staging directory with
canonical names before building the pack.

Stage scraper/title screenshots offline with the `magik-cloud` Rust tool:

```bash
scripts/magik-cloud run -- cargo run -- \
  console-screenshot-stage \
  --system saturn \
  --input data/sources/saturn/originals \
  --output data/sources/saturn/canonical \
  --report work/saturn-stage-report.tsv
```

The report records `mapped`, `unmatched`, `ambiguous`, and `collision` rows.
Resolve ambiguous rows with an overrides TSV of `source_stem<TAB>software_name`;
the runtime still only sees canonical pack entries.

## Runtime Projection

The launcher catalog stores a preview archive path and deterministic asset key
when it has a software-list identity and an archive path for that system. The key
uses the software family when MAME metadata names a parent, otherwise the exact
software name:

```text
mame-software__<list_name>__<family_or_software_name>
```

The database builder does not read screenshot archive indexes and does not prove
that an image exists. Runtime preview loading tries the stored archive/key pair;
if the entry is missing, the preview worker records the failed lookup and the UI
shows the blank preview state. Preview pack changes are not catalog stamp inputs
and do not trigger database rebuilds.

For device acceptance, `scripts/agent device catalog inspect` reports `preview_keys` and
`available_previews` on every `catalog_v3_system_tsv` row. Atari Lynx must have
nonzero values for both after rebuilding with current MAME metadata and the
installed screenshot-pack index.

## Performance Notes

SQLite does not load the full `library.sqlite3` into RAM at startup. The slow
operation is building the database, especially the first identity pass over
console ROM payloads on MiSTer's exFAT/FUSE storage.

Avoid doing screenshot-pack construction on the MiSTer hot path. Runtime deploy
does not build or copy screenshot packs or MAME/HBMAME metadata databases; treat
those as fixed release artifacts produced by the host-side catalog/media tools.

Screenshot-pack updates from Cloudflare R2 are handled by the MagiK runtime,
`scripts/agent device media check`, `scripts/agent device media download --attended`, and the component-selected
`scripts/agent benchmark` media scenario. Runtime v1 uses raw manifest
`compression: "none"` with `Accept-Encoding: identity`. The launcher runtime
queues downloads only for systems discovered by the active catalog scan and
runs one active pack download at a time; the active pack may fetch its small
index sidecar in parallel. Compression comparisons should first use Cloudflare
negotiated responses and recorded header/cache evidence for the canonical
`.mmlz4b` object. Store or select `.gz` or `.br` files only if total time
improves after including download, decompression, saving,
verification, and cache behavior.

Changes to the save/publish path use `scripts/agent benchmark`; structured
events report copy, file sync, rename, parent sync, total time, and progress
event count for the supported progress-capable save path.
