# Fast five-system catalog prototype

The fast five-system prototype is a parallel catalog for Arcade, Amiga, DOS,
X68000, and C64. It does not run the whole-card planner, catalog scanner,
catalog database, resume journal, or Catalog V3 state machinery.

Each system-specific builder produces final UI rows in a small snapshot. The
shared publisher writes the existing immutable SQLite, navigation, and NavPack
artifacts because those are efficient launcher interchange formats, not scan
authority. It then commits a dual-slot manifest. The real launcher maps the
same NavPacks it uses in production, so browsing, search, previews, metadata,
and structured launch plans exercise the actual UI code.

The C64 snapshot groups conservative cross-source title variants before
publication. A local row is grouped only when exactly one OneLoad64 row has the
same title after removing bracketed release metadata and punctuation. OneLoad64
is the visible family representative; language and formatting editions retain
their complete launch records in the C64 SQLite
`fast_five_game_variants` table. They therefore do not occupy NavPack rows or
inflate the instant first-page sidecar. Local-to-local fuzzy matches remain
separate because short or similarly named C64 programs are often unrelated.

The optional `snapshot-reference` command reads only the five systems from an
existing catalog. It is a parity oracle and bootstrap source while independent
AmigaVision, 0MHz, Neon68K, and OneLoad64 release adapters are being completed;
it is not part of a fast build. `replace-arcade` replaces its Arcade rows with
the independent Arcade prototype's ROM-validated active output.

The `scan-generic-examples` command adds four ordinary user-managed systems:
ZX Spectrum, SNES, Neo Geo, and Sega Saturn. This path has no release manifest
and does not read the old catalog. It recursively walks each profile's game
directories, accepts only payloads supported by the installed core, reads ZIP
central directories without extraction, and records direct launch plans.
Saturn cue-track files and other profile-defined support media are excluded.
Neo Geo ROM-set ZIPs are treated as one launchable game rather than expanded
into misleading member rows. Direct files are not opened, hashed, or statted;
the catalog fingerprint covers the path-derived rows because replacing ROM
contents at the same path does not change the catalog or launch plan.

```bash
five-system-catalog-prototype snapshot-reference \
  --catalog-root /media/fat/mister-magik-dev/catalog-v3 \
  --output /tmp/fast-five-reference.json

five-system-catalog-prototype replace-arcade \
  --input /tmp/fast-five-reference.json \
  --arcade-active /tmp/arcade-active.bin \
  --output /tmp/fast-five.json

five-system-catalog-prototype scan-generic-examples \
  --input /tmp/fast-five.json \
  --output /tmp/fast-nine.json \
  --storage-root /media/fat \
  --input-encoding json

five-system-catalog-prototype publish \
  --input /tmp/fast-five.json \
  --output-root /media/fat/mister-magik-dev/fast-five-catalog
```

The parallel catalog is selected for one development launcher process with:

```bash
MISTER_FAST_FIVE_CATALOG=1 \
MISTER_SHARDED_CATALOG_DIR=/media/fat/mister-magik-dev/fast-five-catalog \
MISTER_CATALOG_REFRESH=off \
/media/fat/mister-magik-dev/mister-magik-fb ui
```

Fast-catalog mode accepts either the exact base five or the exact expanded nine
systems and forces catalog refresh off. It intentionally bypasses the
production binding and catalog-state files because the parallel builder does
not create them. It still validates the current shard format and manifest,
maps generation-bound NavPacks, and derives a SHA-256 fingerprint from the
published artifacts.

Timing comparisons are accepted only from isolated builds after a verified
supervised reboot. Each old/new sample must build one named system only. Warm,
forced-rebuild, and whole-card figures are not comparison evidence.

The first exact cold comparison is recorded in
[`history/2026-08-26-fast-five-cold-comparison.md`](../history/2026-08-26-fast-five-cold-comparison.md).
The focused C64 SQLite/FTS experiments are recorded in
[`history/2026-08-26-c64-artifact-experiments.md`](../history/2026-08-26-c64-artifact-experiments.md).
The complete five-system CPU profile is recorded in
[`history/2026-08-26-fast-five-pprof.md`](../history/2026-08-26-fast-five-pprof.md).
The generic ZX Spectrum, SNES, Neo Geo, and Sega Saturn prototype is recorded
in
[`history/2026-08-26-generic-system-catalog-prototype.md`](../history/2026-08-26-generic-system-catalog-prototype.md).

## Optimization experiment matrix

The prototype can publish the same five-system rows through isolated input and
artifact profiles. Snapshot inputs support JSON, Postcard, LZ4-compressed
Postcard, and mmap-backed Postcard access. Artifact profiles independently
exercise removal of the embedded navigation payload, the adjacent navigation
file, both JSON navigation representations, single-pass tmpfs publication,
search-only SQLite storage, and reduced FTS detail.

The attended matrix command gives every profile its own supervised reboot and
clean output root, verifies every row from SQLite identity plus NavPack data,
compares deterministic search fingerprints, and opens the result in the real
Dev UI:

```bash
scripts/agent device catalog fast-five-experiments \
  --attended --reboot \
  --binary crates/catalog/target/armv7-unknown-linux-gnueabihf/release-device/five-system-catalog-prototype \
  --out build/agent-benchmarks/fast-five-experiments/REVISION
```

SQLite remains the persisted FTS/autocomplete database and NavPack remains the
canonical low-latency UI representation. The experiment profiles never alter
the production registry.
