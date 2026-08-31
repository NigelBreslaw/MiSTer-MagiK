# Console Media Identity

MiSTer MagiK uses MAME software lists as the canonical identity source for
console media where practical. Runtime must not depend on scraper screenshot
filenames, `gamelist.xml`, or scanning screenshot folders.

The XML boundary is intentionally narrow: MAME `-listxml` and MAME `hash/*.xml`
software lists are offline inputs to `mister mame-metadata-build`. CI then
converts the source SQLite databases into the compact
`magik-metadata-v1.bin` runtime container. The scanner, launcher, and media
worker read that container, not XML or source SQLite files.

## Target Systems

The supported console identity registry covers the following canonical system
IDs: `nes`, `fds`, `snes`, `n64`, `sms`, `megadrive`, `s32x`, `megacd`,
`saturn`, `atarilynx`, `amigacd32`, `c64`, `zx-spectrum`, `acornatom`, `acornelectron`,
`bbcmicro`, `archie`, `apple-ii`, `apple-iigs`, `amstrad`, `atari2600`,
`atari5200`, `atari7800`, `atari800`, `atarist`, `c128`, `c16`, `pet2001`,
`vic20`, `colecovision`, `megaduck`, `wonderswan`, `wonderswancolor`, and
`x68000`. Amiga uses the dedicated AmigaVision provider; Amiga CD32 uses the
`amigacd32`/`cd32` MAME software-list aliases.

Media-specific MAME lists are retained as release inputs and canonicalized into
these namespaces. For example, `c64_cart`, `c64_cass`, and `c64_flop_*` all
feed the `c64` identity namespace; the complete mapping is enforced by the
catalog and cloud stager rather than by inventing a single source list.

Library identities use:

- namespace: `mame-software`
- identity id: `<list_name>:<software_name>`
- family id: parent software item when MAME has one, otherwise the same item

## Metadata Build

`mister mame-metadata-build` writes the CI/private source database used to
generate the runtime container. The command can read MAME machine XML from
`mame -listxml` or `--listxml`; it reads console software-list XML only from
explicit `--software-list` paths or the target files inside a `--software-dir`
MAME hash directory. Arcade player counts and control types come directly from
each machine's `input` and `control` elements in `-listxml`.

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
package, passes `--software-dir`, and verifies representative multi-media
lists before packaging. The builder ingests every XML in that directory; a
numbered game-database release is required before staging a pack so the cloud
stager can prove that the selected system's mapped source lists contain rows.

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

Console screenshot packs live under `/media/fat/mister-magik/assets/` with
the fixed raster in the filename. Profiles include `arcade 320x320`, `neogeo
320x224`, `nes/fds 256x240`, `snes 256x224`, `n64/saturn 320x240`,
`sms/acornatom/colecovision 256x192`, `megadrive/s32x/megacd/atari7800
320x224`, `amiga/amigacd32 320x200`, `atarilynx 160x102`,
`acornelectron/bbcmicro/archie 320x256`, `apple-ii 280x192`,
`apple-iigs/amstrad/atarist/c64/c128/c16/pet2001 320x200`, `atari2600
160x192`, `atari5200/atari800 320x192`, `vic20 176x184`, `megaduck 160x144`,
`wonderswan/wonderswancolor 224x144`, `x68000 256x256`, and `zx-spectrum
256x192`.

All entries in a pack use its one fixed raster. Atari Lynx and WonderSwan
profiles additionally allow a verified width/height swap for rotated
screenshots; no other geometry is accepted.
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

The launcher catalog stores a deterministic preview asset key whenever it has a
software-list identity. Archive path and `has_preview` are availability fields;
an absent pack must not suppress the key. The key uses the software family when
MAME metadata names a parent, otherwise the exact software name:

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

### Post-download identity reconciliation

The media worker performs a second, runtime-only identity pass after a pack is
installed or confirmed current. It uses one lazily loaded software shard from
the numbered-release `magik-metadata-v1.bin` container and the pack's
`.mmlz4b.idx`; the user's local ROM collection is never treated as the global
identity source. Existing catalog keys have priority, followed by a unique
normalized MAME title/family match whose key is present in the installed pack.
Zero candidates and ambiguous matches remain unresolved.

The resolver opens only the compact header/index, then reads the requested
system shard containing title/family rows. It canonicalizes media-specific
lists (for example, C64 cartridge and cassette lists) before creating
`mame-software__<list>__<family>` keys. Missing or unreadable compact metadata
is non-fatal: exact catalog keys continue to reconcile and the structured
update event reports `resolver_status=Unavailable`. During migration only, a
valid legacy SQLite database may be used as the reported `LegacySqlite`
fallback; full SQLite databases remain CI/private-build source artifacts.

This first tranche intentionally does not perform fuzzy matching, ROM hashing,
or external Libretro, No-Intro, Redump, TOSEC, ScreenScraper, or Skyscraper
lookups. Cartridge checksums, disc serials, specialist DAT imports, and a
persistent generation/pack-keyed overlay remain future extensions.

The read-only device audit runs the same reconciliation without writing the
catalog or downloading media:

```bash
scripts/agent device catalog screenshots --system nes --out work/nes-screenshots.tsv
```

It prints a `catalog_screenshot_summary_tsv` record separately from the TSV,
including total games, existing and derived identities, ambiguous rows,
available rows, and resolver status. The TSV contains the effective runtime
columns `ordinal`, `title`, `preview_asset_key`, `preview_archive_path`,
`has_preview`, and `launch_ref`.

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
