# Prepared bundle-helper research

Date: 2026-08-26

## Scope

This review covered the four sister projects already recognized by the
production collection adapters: AmigaVision/MegaAGS, 0MHz, Neon68K, and
OneLoad64. No payloads were downloaded, launched, changed, or copied. Device
evidence came from typed read-only inventory and Catalog V3 queries. The user
backup at `/Volumes/backup` was inspected read-only.

## Public release evidence

| Collection | Public release evidence | Shape relevant to helpers |
|---|---|---|
| AmigaVision / MegaAGS | The official news and repository identify `2026.04.26` (including repository tag `2026.04.26-r2`) as the latest published release found. The release reports 5,342 hand-tuned configurations. | One generated HDF installation with game/demo listing files and MiSTer launcher MGLs. Hash the small listings/MGLs; size-check the HDF payloads. |
| 0MHz | The official site describes a pick-and-choose collection. The tagged `2024.03.20` repository tree has 181 MGLs; current `main` has 327. | This is not one all-or-nothing bundle. Match per-game MGL and payload receipts, then scan unmatched games. |
| Neon68K | The only official announced release found is `2025.04.29`, describing the 2025-04-28 archive. | Per-game MGL/HDF packages. Match a release MGL inventory and payload receipts. |
| OneLoad64 | The official site identifies version 5 and reports more than 2,100 games. No newer packaged release or repository tag was found. | A named versioned directory of primary/MultiLoad64 CRTs. Match only the primary launchable inventory and retain exclusion rules. |

Official sources:

- <https://amiga.vision/2026.04.26>
- <https://github.com/amigavision/AmigaVision>
- <https://0mhz.net/>
- <https://github.com/0mhz-net/0mhz-collection>
- <https://neon68k.com/2025.04.29>
- <https://oneload64.github.io/v5>

## Installed device evidence

The retained cold inventory is
`build/agent-benchmarks/catalog-corpus-inventory/1787738772`.

| Collection | Installed candidates | Catalog rows recognized | Assessment |
|---|---:|---:|---|
| AmigaVision | Installed HDF/listing layout present | 1,541 games, 206 demos, plus launcher | Not the current published content set: the official current setup advertises more than 3,000 games and almost 600 demos. Keep scanner fallback. |
| 0MHz | 305 MGLs | 237 launchable rows | A valid partial/custom selection, not a complete current `main` manifest. This is expected for a pick-and-choose project. |
| Neon68K | 273 MGLs | 268 launchable rows | The complete 273-name 2025-04-28 release manifest is installed, but five entries do not pass current launch validation and must not be supplied blindly by a helper. |
| OneLoad64 | 2,667 CRT candidates in the broader C64 tree | 2,219 prepared rows under `OneLoad64-v5` | Version 5 is installed and matches the latest packaged version found. Exact helper identity still needs a manifest/receipt rather than trusting the directory name. |

The backup contains `neon68k/Neon68K-20250428` with 273 packages for each of
the two video setups. Its metadata identifies version `20250428`; the MiSTer
Upscaler filename manifest hashes to
`cd7882e056f102840bfb31412f79889569736b248b69df3e4f2f3e5a85b7fb1e`.
That exactly explains the device's 273 Neon68K MGL candidates.

## Prototype safety contract

The helper prototype stores precomputed rows and three kinds of evidence:

1. SHA-256 receipts for small authoritative metadata such as listings and MGLs.
2. Size/existence receipts for large user-owned payloads.
3. Optional inventories of launchable paths to detect additions and removals.

An exact match returns precomputed rows. Any missing payload, changed metadata,
extra game, removed game, old version, partial set, custom set, path collision,
or unreadable evidence calls the normal scanner. The fallback result, not the
helper, is authoritative in that case.

The prototype deliberately does not publish Catalog V3 artifacts yet. A future
integration must feed exact helper rows through the existing SQLite/NavPack
projection so the launcher's first-page and scrolling behavior remain intact.
