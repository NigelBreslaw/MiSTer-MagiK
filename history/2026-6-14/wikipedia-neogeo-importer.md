# Wikipedia Neo Geo screenshot importer

`scripts/import-wikipedia-neogeo-screenshots.py` is a prototype-only/local-only
helper for testing the screenshot-pack system. It is not a production asset
source.

The importer uses the MediaWiki API for:

```text
Category:Screenshots_of_Neo_Geo_games
```

It writes:

```text
build/neogeo-screenshots/wikipedia/
  originals/
  mapped/
  report.tsv
```

Usage:

```bash
scripts/import-wikipedia-neogeo-screenshots.py --mame-db build/mame.sqlite3
```

Dry mapping run:

```bash
scripts/import-wikipedia-neogeo-screenshots.py --no-download --limit 20
```

Self-test for filename normalization:

```bash
scripts/import-wikipedia-neogeo-screenshots.py --self-test
```

Mapped images can be packed by pointing the Neo Geo pack builder at the mapped
directory as local originals:

```bash
scripts/build-neogeo-screenshot-pack.sh \
  --skip-fetch \
  --work-dir build/neogeo-screenshots/wikipedia-pack
```

Copy or link `wikipedia/mapped/*` into
`wikipedia-pack/originals/` before running the pack builder.

Ambiguous titles are reported in `report.tsv` and are not guessed. This keeps
the prototype useful for testing without baking bad identity matches into a
pack.
