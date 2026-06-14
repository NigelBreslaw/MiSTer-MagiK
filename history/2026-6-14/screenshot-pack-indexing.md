# Screenshot pack indexing

Screenshot archives are now treated as v1 asset packs in `library.sqlite3`.

## Pack format

The existing compressed raw565 archive remains the production format:

- magic `MMLZ4B1\0`
- index entries named by setname stem, such as `1941u.rgb565`
- LZ4 block payloads

Raw packs (`MMRAWP1\0`) use the same index shape. The catalog reads only the
index when building the library database; it does not decode preview payloads.

## Database rows

`asset_packs` stores the local archive path, platform, asset type, codec, and
version. `asset_entries` stores each setname stem as a MAME identity and records
its family id using MAME metadata when available.

The arcade/Neo Geo projections expose:

- `asset_pack_id`
- `asset_key`
- `asset_link_reason`

`asset_link_reason` is one of:

- `exact`: the launchable identity has its own screenshot.
- `parent`: the MAME parent has a screenshot.
- `sibling`: another machine in the same family has a screenshot.
- `none`: no pack entry could be linked.

This is the intended replacement for `gamelist.xml` screenshot inheritance. A
clone like `1941j` can display `1941u.png` when that is the only available family
screenshot, without filename/title guessing.
