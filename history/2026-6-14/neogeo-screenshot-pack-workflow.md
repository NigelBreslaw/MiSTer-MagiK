# Neo Geo screenshot pack workflow

`scripts/build-neogeo-screenshot-pack.sh` builds a local Neo Geo screenshot pack
from screenshots already installed on the MiSTer.

Default source:

```bash
/media/fat/games/NEOGEO/screenshots
```

Default local work tree:

```bash
build/neogeo-screenshots/
  originals/
  cache/png-hybrid-320x320/
  cache/raw565-hybrid-320x320/
  cache/raw565-hybrid-320x320-lz4block-12.mmlz4b
```

Build locally from the MiSTer source images:

```bash
scripts/build-neogeo-screenshot-pack.sh
```

Rebuild from already fetched originals:

```bash
scripts/build-neogeo-screenshot-pack.sh --skip-fetch
```

Build and deploy the pack:

```bash
scripts/build-neogeo-screenshot-pack.sh --deploy
```

Default deploy path:

```bash
/media/fat/mister-magik/assets/neogeo-screenshots.mmlz4b
```

The script uses the existing host conversion path:

1. `scripts/mister get` copies source PNG/JPG files to `originals/`.
2. `scripts/mister preview-cache-build` writes resized PNGs, `.rgb565`
   previews, and the compressed LZ4 block archive.

The filenames should already be MAME setname stems for the MiSTer Neo Geo pack,
for example `mslug3.png`, `kof98.png`, and `aof2.png`. Those stems line up with
the MAME identities used by the library database.

Runtime use of multiple screenshot packs is completed by the deploy/assets
integration slice; this script only creates the pack artifact.
