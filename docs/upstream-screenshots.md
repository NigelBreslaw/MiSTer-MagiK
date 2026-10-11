# Upstream Arcade screenshot audit

Run from the repository root:

```sh
scripts/magik upstream-screenshots
```

This host-only command discovers core/distribution feeds from Update All's current
`AllDBs` registry, including unofficial providers. It reads all MRA entries before
applying any downloader release filters, so beta and alpha games are included.
It does not connect to MiSTer, install games, run Update All, or download ROMs or
cores. `curl` and the authenticated local `gh` CLI are required.

The audit decodes each feed's numeric release tags, including Jotego `jtbeta` and
Coin-Op's beta/alpha tags. For GitHub-backed feeds with a pinned source URL it
also scans that exact Git tree for MRAs omitted from the downloader manifest,
such as alternatives. Untagged tree supplements are labelled `unknown`, not
silently assumed public. Asset, names, ROM and manual databases are excluded;
new providers in the registry's core/distribution sections are discovered
automatically. Additional feeds outside Update All can be included with
`--database-url https://...` (repeatable).

MRA contents are verified against manifest MD5/size or Git blob SHA-1/size.
The published Arcade screenshot index is verified against the public manifest's
SHA-256, byte size and archive identity. Coverage counts parent/family images,
not individual clone, bootleg or hack setnames. It uses MAME/HBMAME relationships,
reviewed custom-family links, explicit MRA parents, then an unambiguous known
family from the MRA's ROM archives. BIOS/device archives do not establish game
families. It never guesses from a similar title, core name or shared hardware.
A published clone image can represent its known family; `matched_key` records
which existing image is reusable. The verified index also checks family storage:
compatibility keys sharing one payload are aliases; different stored payloads
within one known family are reported as redundant family images. This preserves
older catalog keys without storing multiple screenshots of the same family. Unknown or conflicting external-ROM families
are reported as `family_unresolved`, rather than counted as new image requests.
Embedded games with an explicit setname retain their own identity. MAME BIOS,
device and software-list platform records are retained as `not_game`, separately
from game screenshot requests (for example, PGM BIOS and NeoGeo Pocket). The latest MAME
release's listxml is downloaded by default; use
`--mame-listxml PATH` for a local XML or ZIP. HBMAME parents come from the latest
published game-database source artifact in this repository, verified against its
GitHub artifact SHA-256 and byte size. Use `--hbmame-database PATH` for a local
read-only SQLite source if the artifact is unavailable or expired. Provenance for
both parent maps is recorded in `audit.json`. A blank/invalid setname is reported
as `identity_unresolved`, separately from a definite missing screenshot.

Results go to ignored `build/arcade-upstream-audit/` by default:

- `report.md`: readable summary, missing beta/alpha games, all missing games,
  unresolved identities, and source coverage/errors.
- `missing.tsv`: names, screenshot keys, release channels, source feeds and
  exact MRA URLs for missing/unresolved games.
- `all-games.tsv`: includes covered families as well.
- `family-links.tsv`: one row per source MRA, retaining its original setname,
  resolved family, reusable published key, resolution method and evidence URL.
  This makes clone-to-family links explicit, including beta/alpha variants.
- `crates/catalog/data/arcade-screenshot-families.tsv`: reviewed exceptions for
  custom MRA keys absent from machine metadata, with pinned evidence URLs.
  The current table retains all 90 custom links established by the audited MRAs. The
  catalog and audit share this file; updating it links a custom variant to its
  family instead of adding a duplicate screenshot.
- `audit.json`: complete structured results and source/pack provenance.
- `snapshot.json`: last complete audit, used automatically as the next run's
  baseline. `new_game` and `new_gap` identify changes; they are false on the first
  run. An incomplete run does not replace the last complete baseline.
- `cache/`: downloaded metadata and verified MRA bytes, reused for immutable
  objects. Feed manifests, the registry and the screenshot manifest refresh on
  every run.

Use `--output PATH` for another output/cache directory, `--baseline audit.json`
to compare against a selected complete audit, or `--workers N` (1–16) to bound
parallel downloads. Counts group clones and reviewed variants by screenshot family, preserving
all source paths, titles and channels; one group can appear in multiple channels.

Exit code 0 means the audit completed, even if screenshots are missing. Exit code
1 means the report is partial because a feed, supplemental source tree or MRA
could not be inspected. Exit code 2 means required registry, pack, MAME metadata
or command configuration failed. A source failure is never treated as proof
that a game has disappeared. Early warning begins when accessible upstream MRA
metadata is published; unannounced/private games cannot be inventoried.
