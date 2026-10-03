# Bridge refactor and recalculation audit

The bridge now publishes cached user data. It does not scan game collections to
compute recent or favourite counts. The implementation is rebased onto
`6fc86fe7f` (PR #219), with a fresh native baseline and two comparison builds measured on that same
base. Dev was reserved exclusively for these completed campaigns.

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

## Rebased native comparison

Each build completed three instrumented Consoles → Nintendo → SNES hub → Games
→ return journeys on PR #219. Hub bridge values below are the maximum retained
system-hub bridge batch in each journey; they are not whole-route cumulative
times. CPU is whole-process CPU per moving presentation.

| Build | Hub bridge, three repeats | Recent + favourite reads | Route drops | Mean moving CPU |
| --- | --- | --- | --- | --- |
| Instrumented baseline | 12.667 / 13.120 / 12.902 ms | 11.989–12.600 ms combined | 7 / 6 / 9 | 18.355 ms |
| Maintained counts | 0.534 / 0.640 / 0.464 ms | 5–6 us combined | 6 / 6 / 5 | 18.520 ms |
| Full audited refactor | 0.470 / 0.424 / 0.423 ms | 5–6 us combined | 10 / 7 / 5 | 18.349 ms |

Mean retained hub bridge cost fell from 12.896 ms to 0.439 ms, a 96.6% reduction.
Every final sample meets the 0.5 ms target. Baseline and final each dropped 22
frames across three routes. The CPU difference is negligible; neither an overall
CPU reduction nor a frame-drop improvement is established. The counts-only
drop result is too small a sample to establish a separate benefit. Remaining
rendering, pacing and latch deadlines still need work before claiming 60 fps
with zero drops. These phase-instrumented routes do not establish uninstrumented
performance.

All nine measurement journeys passed, with zero evidence retention overflow,
zero latch rejections and matched physical latch posts/flips. Evidence retention
is selective; the bridge figures describe the captured hub-entry batches, not
every frame of a route.

| Build | Source revision | Native measurement run |
| --- | --- | --- |
| Baseline | `9373aaad8` | `20261003T165857Z-20c283680d67` |
| Maintained counts | `cf9ba1520` | `20261003T170616Z-d3624b075c59` |
| Final | `b2cd60791` | `20261003T171454Z-ae956ce1cde0` |

Final installed executable SHA-256:
`a18064e8ce99222d87fed68fba9e7965a6c920de51a271e90467b9ea10b3e691`.

## Validation and boundaries

Rebased checks pass: 173 navigation tests, 24 bridge tests, user-state worker
sequences, persistence/import and transaction rollback tests, lazy reference
resolution, launch-plan replacement, UI-only Clippy and catalog Clippy. Rust LSP
diagnostics are clear in the managed worktree. The final rebased ARM release
build passes.

Baseline and final native control/visual journeys passed
(`20261003T170312Z-62ccb4dd77b0` and `20261003T172017Z-9b61bcae530d`).
They exercise A, Back, the Select action queue and a tap during the locked trick.
These are Dev controls, not physical joystick qualification. Captures come from
`fpga-latched-scanout-slots`, RGB565, 960×540. CRT/portrait output was not qualified.

After excluding the clock and game-preview rectangles, Games and root captures
are pixel-identical. Hub and Select-return hub differ in 547 pixels, confined to
the favourites count glyph: baseline shows 0, final shows 1. This is not a
zero-difference visual result. The new count reads stored per-system identities;
the old count intersected references with installed catalogue rows. The device
journey does not independently verify that stored identity against the installed
game list. Layout and artwork otherwise match in these settled captures.

Raw runs, build logs and the aggregated comparison remain ignored under
`build/magik-results/` and `outputs/bridge-refactor/`. Old pre-PR #219 measurements
above are provenance only and are not mixed into this comparison.

## Deletion and simplification audits before PR

Removed the uncalled global full-metadata query APIs and duplicate count reader;
backfill tests now exercise the production snapshot read. Removed the unused
play-refresh mode and catalog parameters from maintained-count getters. Playback
still updates both MRU indexes and counters through the existing SQL triggers;
the running worker only refreshes favourites, and launch return reads a new
startup snapshot.

Favourite refresh now completes both queries and the read transaction before
changing the cached projection. A failed count read leaves the old snapshot
intact. This allows the worker to update its cache directly and copy it once for
publication. A database failure/retry sequence tests that guarantee. Removed the
hub cache's collection ID, which did not affect any cached output, and unused
Settings key derives/duplicate global access.

The app benchmark now owns its route configuration and navigation in one module;
the separate route module and circular import were removed. Inputs, wait policy,
measurement windows and destination assertions retain the measured workloads.

Final focused validation passes: 173 navigation tests, 24 bridge tests, 6 worker
sequences, 11 persistence/import tests, 14 host benchmark/CLI tests, UI and catalog
Clippy, and Rust LSP diagnostics. These final API/cache/harness cleanups were
validated on the host; the native measurements above refer to `b2cd60791` and
were not repeated after cleanup. Card artwork, geometry and FrameClock behavior
were not edited in either audit.

## PR review fixes: saved-list catalog and Arcade membership

Saved-list indices now track the catalog projection independently of user-data
changes. Catalog replacement and hydration rebuild those row positions, including
when the user snapshot is unchanged. Membership and counts use the actual
`menu:arcade` view; NeoGeo activity in the separate SNK collection is excluded
from initial and incremental Arcade summaries.

The worker reads Arcade MRU references across the collection's member systems,
filtering membership before ordering and the 16-entry limit. Console history can
fill global recents without hiding older Arcade plays. A catalog membership
change refreshes that cached projection. Membership discovery reads mapped system
IDs without materializing game rows. Count getters remain cached scalar reads.

Regression validation covers saved games across row insertion and hydrated row
reordering, legacy Arcade plus CPS1/CPS2/System16 plays followed by 17 console
plays, NeoGeo exclusion, incremental favourite changes, worker membership changes
and lazy mapped-row membership discovery. Host validation: 176 navigation,
24 bridge, 7 worker and 12 persistence/import tests plus the lazy-row regression
and frontend/catalog Clippy. These correctness fixes have not been measured on
MiSTer; the earlier performance results remain tied to their recorded build.
