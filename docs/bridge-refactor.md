# Bridge refactor and recalculation audit

The bridge now publishes cached user data. It does not scan game collections to
compute recent or favourite counts. The implementation is rebased onto
`6fc86fe7f` (PR #219), whose pacing and scheduling changes require a fresh native
baseline before attributing frame-drop changes to this work.

## Measured cause

The instrumented native baseline (`20261003T150546Z-6dfc57eb2379`) identifies the
two former count scans as almost the entire slow hub-entry batch:

| Repeat | Bridge | Recent scan | Favourite scan | Presenter stages: navigation, settings, menu, Arcade |
| --- | ---: | ---: | ---: | --- |
| 0 | 13.304 ms | 10.455 ms | 2.246 ms | 12.760 / 0.029 / 0.073 / 0.062 ms |
| 1 | 12.715 ms | 10.079 ms | 2.112 ms | 12.246 / 0.030 / 0.084 / 0.061 ms |
| 2 | 13.100 ms | 10.442 ms | 2.147 ms | 12.643 / 0.028 / 0.091 / 0.051 ms |

Count timings are nested within navigation, not additive to the presenter
stages. Churn counters are explicitly enabled for measured presenter calls;
they cover instrumented model operations, not general/catalog allocation.

One completed counts-only native route (`20261003T161719Z-3a93a3b8b542`, repeat 0)
measured a 657 us hub bridge and 5/2 us recent/favourite reads. It had 18 route
drops and 18.5754 ms process CPU per moving presentation. The next repetition
failed `artifact-superseded` after another publication changed the installed
artifact. This is one valid route, not a completed repeated comparison. It also
predates PR #219. No zero-drop or mean-CPU improvement is established.

## Data ownership

Existing favourites and play sessions already carry system and game identities.
Additive derived tables maintain one current recent row per identity and
per-system counters. SQLite triggers update history, MRU and counters within the
existing write transaction. Replaying a game updates its MRU position/play count
without increasing its unique-game count. Favourite insert/delete increments or
decrements the relevant counter; idempotent writes do not double count.

Existing schema-v1 writers remain compatible. Existing history is preserved and
backfilled once when derived tables are absent. Global and per-system MRU reads
use indexes on the maintained rows; the previous correlated history queries are
removed. Per-system recent lists remain independent of the global recent limit.
Counts represent stored distinct played/favourite identities; displayed MRU lists
remain bounded to 16 entries.

The background user-state worker reads one coherent startup snapshot and updates
only the affected system after a favourite mutation. It publishes data for all
systems instead of the previous SNES-only snapshot. Startup legacy identity
resolution runs on this worker, not in the launcher loop. A known selected row
also avoids whole-library reverse lookup for favourite actions.

## Audit actions

| Finding | Result |
| --- | --- |
| Two collection scans for hub counts | Replaced by reads of maintained user-state counters. |
| Correlated play-history reconstruction | Replaced by maintained MRU rows and indexed queries. |
| SNES-only worker publication | Global and per-system snapshots published. |
| UI-thread legacy identity rebuilding | Moved to the existing worker; startup work is cached. |
| Unchanged user snapshots causing bridge/list refresh | Publication compares cached user data and skips redundant updates. |
| Duplicated settings projection | One presenter owns settings; unchanged inputs skip publication, including card-selection ticks. |
| Hidden Home menu rebuilds | Deferred until Home is visible again; feedback retirement remains cross-view. |
| Hub performing list/search/drawer projection | Shared system state is published; list-specific work waits for the list. |
| Repeated hub string formatting | Title, subtitle and section captions cached until metadata/counts change. |
| Saved-list metadata resolution materializing unrelated games | Reference-only access preserves lazy mapped game rows. This is metadata resolution on list/data changes, not a count algorithm. |
| Opaque zero churn counters | Enablement is explicit; coverage limits remain documented. |

Native artwork, rotations, animation durations and FrameClock progression are
unchanged. Destination fields still publish before destination raster/capture.
Display/orientation confirmation countdowns and display-geometry fallback remain
covered by state-sequence tests. Device images remain prepared/cached when their
kind becomes known, so view scoping does not defer expensive pixel construction
to the hub's first raster.

## Validation and outstanding native work

Rebased checks pass: 172 navigation tests, 24 bridge tests, user-state worker
sequences, persistence/import and transaction rollback tests, lazy reference
resolution, launch-plan replacement, UI-only Clippy and catalog Clippy. Rust LSP
diagnostics are clear in the managed worktree. The final rebased ARM release
build passes.

Outstanding: exclusive Dev availability, a fresh instrumented baseline on PR
#219, repeated counts-only/final measurements, and the native control/visual
journey. Old measurements must not be mixed with the new base when estimating
frame-drop changes. Raw runs and build logs remain ignored under
`build/magik-results/` and `outputs/bridge-refactor/`.
