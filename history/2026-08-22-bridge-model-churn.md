# Bridge-model churn baseline

Date: 2026-08-22

## Scope

This is the exact-device baseline for roadmap Item 13 and the authority for the
Item 14 experiment. The fixed route exercised the production bridge with 60
media-progress publications, 64 selections in a retained 128-row menu, and 64
unchanged light bridge revisions. Every update waited for physical
presentation. Production defaults remained unchanged.

Installed revisions:

- MiSTer MagiK: `f9c185061802252b5aec2f1671d8d60f65b80dfe`
- Main_MiSTer: `639d3694e1b93660020e9587cd0fe27f0170ce4c`

Artifact:
`build/agent-benchmarks/bridge-model-churn/1787375759/summary.json`

## Implementation and verification checklist

- [x] Played fixed media-progress events through `MediaProgressDisplay` and the production worker UI intent.
- [x] Included immediate completion and failure terminal events.
- [x] Played selection changes through a retained 128-row production Slint model.
- [x] Verified one selected row and zero spurious acknowledged rows.
- [x] Played fixed unchanged light bridge revisions.
- [x] Counted model replacements, row mutations and allocations, and `SharedString` constructions.
- [x] Recorded model-allocation, bridge, raster, damage, copied-byte, and cadence results.
- [x] Verified terminal progress rows and summary text.
- [x] Cleared benchmark media and republished the real menu model before completion.
- [x] Restored exact semantic Home state, display mode, ordinary launcher, manifest, and boot identity.

## Exact-device results

| Phase | Updates | Replacements | Row allocations | Row mutations | Shared strings | Allocation | Bridge | Raster | Bridge+raster | Full-damage frames | Copied bytes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Media progress | 60 | 60 | 177 | 0 | 1,902 | 1,783 us | 5,432 us | 712,427 us | 717,859 us | 59 | 108,860,800 |
| Menu selection | 64 | 2 | 256 | 126 | 1,216 | 608 us | 6,219 us | 528,143 us | 534,362 us | 1 | 6,829,488 |
| Light bridge | 64 | 0 | 0 | 0 | 832 | 0 us | 2,277 us | 481,398 us | 483,675 us | 1 | 1,843,200 |

Per-update bridge+raster costs were 11,964.32 us for media, 8,349.41 us for
menu selection, and 7,557.42 us for light sync.

The media terminal summary was
`screenshots 0 active · 2/3 done · 1 failed`; all three rows were retained with
their final percentages. The menu terminal state contained 128 rows with only
row 63 selected and no acknowledged rows. Restoration required three model
publications and left zero media rows, an empty media summary, and the real menu
model active.

The terminal Home PNG SHA-256 was
`2a276b872e27d1ad694b23ce7ca1fbf8636721b31db1709ac732e620ad49bde6`.
Latch drops, sequence gaps, ownership losses, and repeated physical vblanks
were all zero.

## Recent-history disposition

The menu-content half of Item 14 has already been partly realized by the
retained content/presentation work in `acda461ab`, `5ec786f9e`, and related
selection-projection commits. The baseline confirms there is no per-selection
model replacement or allocation to remove. The remaining menu opportunity is
the O(128) scan used to find the two rows whose selected state changes.

The dominant measured opportunity is media progress: every update replaces the
model, reconstructs rows and strings, and causes nearly every frame to carry
full damage. The secondary opportunity is revision-stable light sync, which
constructs 13 unchanged `SharedString` values per update. Item 14 should retain
and diff media rows, preserve immediate terminal publication, coalesce only
nonterminal progress, update only previous/current menu rows, and cache light
strings by source revision. The 80% replacement/allocation and 10%
bridge+raster gates are now measurable against this artifact.
