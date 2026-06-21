# First-Boot Scan Screen

## Context

Reset Database deleted `/media/fat/mister-magik/library.sqlite3` and rebooted.
The deployed Main fork generated a launcher script that ran
`mister-magik-fb library-refresh` in the foreground when the database was
missing, then started `ui launcher 0`. That produced a black HDMI screen while
the index was built.

## Device Evidence

Device: `192.168.1.117`, production boot through `main=MiSTer_MagiK`.

- Foreground pre-UI refresh reproduced the black screen. Process list showed
  `/media/fat/mister-magik/mister-magik-fb library-refresh` instead of
  `ui launcher 0`.
- Foreground refresh timing from `/tmp/mister-magik-library-refresh.log`:
  `scan_us=66839137`, `import_us=28261123`, total about 95.1s.
- Initial visible-UI scan fix worked but was too slow because the first-scan
  worker inherited `lower_background_priority()`: `scan_us=312156412`,
  `import_us=110427245`, bridge sync at `439463ms`.
- Final fix keeps first scans at normal priority and only lowers priority for
  cached background validation. Reboot with missing DB showed the Slint scan
  screen immediately and completed with `scan_us=71261850`,
  `import_us=27626418`, bridge sync at `103437ms`.
- Final database count: `9959` games. Launcher returned to Home at about 59fps.

## Current Policy

- `library-refresh` defers foreground work when launched by Main with
  `MISTER_MAGIK_PARENT` and no database exists.
- Missing/empty catalog scans are owned by the launcher UI worker and must show
  a full-screen scan state.
- First scans run at normal priority. Background validation with a usable cached
  catalog can run at lower priority.
- Cold first-scan counters start from real launcher bootstrap counts and remain
  monotonic. The bootstrap counter is paced slowly so it keeps moving while the
  full metadata scanner catches up; the full scanner takes over only after its
  real count exceeds the displayed bootstrap count.

## 2026-06-21 Counter Bootstrap Follow-Up

Device: MiSTer at `192.168.1.117`, production `MiSTer_MagiK` supervised boot.

The important UX metric is not the first `Games found: 50` message by itself,
but the point where the counter has enough real target behind it to keep moving.
`BOOTSTRAP-50-NOWALK-20260621` proved the first real count at about 1.48s, but
then visibly stalled until the full scanner reported its first 50 discoveries at
about 8.20s.

The follow-up lets the cheap direct `_Arcade` bootstrap count continue past 50
and sends the final real bootstrap count to the launcher. The chosen displayed
counter behavior is paced, not instant: bootstrap counts animate at 55 games/sec
toward a 1000-game floor, and classification updates are ignored until they
overtake the currently displayed number. The device run
`BOOTSTRAP-SLOW-55-20260621` measured:

- first visible frame: 1.185s,
- first meaningful counter target (`Games found: 50`): 1.198s,
- sustained bootstrap target: 1.648s with target 850,
- full classifier first small batch: 8.165s, ignored for visible handoff until
  it overtakes the displayed count,
- library ready: 58.965s with 7221 UI games.

## True Percent From The Beginning

The current scanner streams candidates from the filesystem walker into the
classifier. It does not know the final candidate count until discovery has
finished, so startup progress is intentionally indeterminate while live counters
advance.

A real percentage from the first scan frame would require one of:

- A pre-count/pre-discovery pass over all roots before classification.
- A persisted sidecar manifest from an earlier scan, which does not help true
  first boot or after a full database/cache reset.
- A scanner redesign that separates discovery into a completed manifest phase,
  then classifies against that manifest.

The extra wall time for the pre-count approach is approximately one complete
candidate discovery walk. Existing `results-library.tsv` benchmark rows showed
`discover_us` around 9-16s in the standalone library benchmark, but the exact
production reboot path measured a 66-71s scan phase on this device/library. For
planning, assume true percent from the beginning would add roughly one extra
discovery pass: best case about 10-20s on a warm/benchmark path, worst/current
boot path plausibly about 60-70s. It would also delay useful percentage display
unless an indeterminate "counting games" phase remains.
