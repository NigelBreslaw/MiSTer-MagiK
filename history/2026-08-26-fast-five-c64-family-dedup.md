# Fast-five C64 family deduplication

Date: 2026-08-26

The fast-five C64 snapshot now removes conservative OneLoad64/local title
duplicates from the visible game list before NavPack publication. Matching is
cross-source only: exactly one OneLoad64 row must share the same title after
bracketed release metadata and punctuation are removed. Local-to-local fuzzy
matches remain separate. A language marker such as `de`, `fr`, or `it` is
recorded as a language edition.

Every removed row remains in the C64 SQLite `fast_five_game_variants` table
with its family key, title, launch reference, relation, and complete serialized
game/launch plan. The real UI sidecar therefore contains only visible families
without deleting alternate launch data.

## Reboot-cold device result

Final evidence:
`build/agent-benchmarks/fast-five-c64-family/working-tree-final.json`

Comparison baseline:
`build/agent-benchmarks/fast-five-experiments/ec560da23-combined.json`

| C64 search-only artifact | Before | After | Change |
|---|---:|---:|---:|
| Visible NavPack rows | 15,089 | 14,991 | -98 |
| Retained SQLite variants | 0 | 98 | +98 |
| NavPack | 5,158.0 KB | 5,124.5 KB | -33.5 KB |
| SQLite | 11,828.0 KB | 11,760.0 KB | -68.0 KB |
| SQLite + NavPack | 16,986.0 KB | 16,884.5 KB | -101.5 KB |

The final Postcard-mmap/search-only publication completed in 7.721 seconds,
verified all 17,980 visible rows and all 98 retained variants exactly, and was
opened by the real Dev UI as a five-system catalog. This is one cold result,
not a new timing comparison sample set. Device fault/reboot arming was clear
after the run.

The rule deliberately does not guess translated titles with unrelated
spellings. Those require authoritative release identity or payload metadata;
merging them by generic fuzzy similarity would hide unrelated same-name C64
programs.
