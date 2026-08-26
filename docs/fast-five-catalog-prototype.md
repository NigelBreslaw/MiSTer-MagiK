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

The optional `snapshot-reference` command reads only the five systems from an
existing catalog. It is a parity oracle and bootstrap source while independent
AmigaVision, 0MHz, Neon68K, and OneLoad64 release adapters are being completed;
it is not part of a fast build. `replace-arcade` replaces its Arcade rows with
the independent Arcade prototype's ROM-validated active output.

```bash
five-system-catalog-prototype snapshot-reference \
  --catalog-root /media/fat/mister-magik-dev/catalog-v3 \
  --output /tmp/fast-five-reference.json

five-system-catalog-prototype replace-arcade \
  --input /tmp/fast-five-reference.json \
  --arcade-active /tmp/arcade-active.bin \
  --output /tmp/fast-five.json

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

Fast-five mode rejects any registry that does not contain exactly the five
expected systems and forces catalog refresh off. It intentionally bypasses the
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
