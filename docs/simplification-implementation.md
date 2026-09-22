# Simplification implementation checklist

Base: `3b8d2c3f2d38278b9319fadcddaa40ccb477901d` (`main`, including #151/#152).
Branch: `nigel/project-simplification`. Desktop is retained unchanged.

The approved audit is implemented as bounded logical commits below. Larger
rewrites without demonstrated redundancy were deliberately excluded. Each entry
records the actual scope, including behavior and safety boundaries retained.

- [x] 1. Remove unreachable direct Main execution (`09b169606`). Preserve
  supervised launch and menu boot routing; 11 focused routing tests pass.
- [x] 2. Remove unused monolithic catalog APIs (`93365b851`). Remove unused
  path helpers, MAME/HBMAME path fields, and obsolete structured progress
  constructors. Retain sharded storage, user-state SQLite, predecessor cleanup,
  and the legacy display-progress adapter. All 280 catalog tests pass.
- [x] 3. Prune stale runtime controls (`e90b71d2e`). Remove 42 controls and fix
  the live Arcade root owner. Validate owner existence and exact source-name
  references; regenerate documentation. The validation follow-up runs this
  check in app CI and checks the real registry in Python tests.
- [x] 4. Retire the standalone particle showcase (`105bddbc7`). Remove
  `apps/framebuffer-lab`, its assets, and its experiment documentation and hook
  entries. Retain the shared particle engine, focused scene lab, and runtime
  `framebuffer-lab` feature still used by that lab and Mini-MagiK.
- [x] 5. Retire unfinished effects and experimental transition variants
  (`d6ee8da1f`). Remove experiment dispatch, effects, HUD, and feature wiring.
  Keep production fade/cut rendering and live benchmark scenes; `ui-preview`
  still enables those scenes. Remove one additional experiment-only control
  (43 total). Eight transition, 16 renderer, and 11 command tests pass;
  `ui,bench-scenes` binaries and `ui-preview` compile.
- [x] 6. Remove the orphan Jersey10 license sidecar (`02ce5ef57`). Other font
  notices are retained.
- [x] 7. Remove the unused Rust diagnostics mirror (`5d1bfc9fa`). Remove the
  isolated crate, Rust-only generator output, and obsolete CI/cache/input
  wiring. Preserve schema validation, opcodes, RTL, and generated SystemVerilog.
  Protocol checks, existing FPGA simulations, cache tests, and 20 component-ID
  tests pass. Component cache identity changes; no hardware was deployed.
- [x] 8. Remove reviewed archived raw history (`a0c5c773d`). Retain historical
  summaries, referenced compile reports, and compact catalog qualification
  tables. Each removed artifact was compared with its base-commit bytes;
  `history/README.md` records original paths, SHA-256 hashes, sizes, and recovery
  instructions. Git history is unchanged.
- [x] 9. Simplify architecture reporting (`80746a589`). Remove the retired
  SQLite hotspot and unreliable brace-counted function-size metric; filter
  archived/generated paths from concentration totals. Version the JSON schema
  as v2. Reporter and CLI tests pass.
- [x] 10. Simplify launcher configuration and diagnostics. Benchmark defaults
  use the existing capture parser (`cd9c2d6c3`); six tests pass. Full and partial
  trace snapshots share metadata construction (`9c26e4539`); eight trace tests
  pass, including partial-snapshot completion boundaries. No diagnostic records
  or counters were removed.
- [x] 11. Simplify launcher orchestration through existing owners (`b7805675f`).
  Deferred catalog startup owns its existing start request instead of copying
  its fields. All 23 session tests pass. Readiness, first-frame and delay gates,
  and the #151/#152 rendering pipeline remain unchanged. No frame-order rewrite.
- [x] 12. Simplify connection reporting (`bafc12a9b`). Consolidate native
  connection events and explicit authentication-error handling; all 21
  discovery/compatibility tests pass. Discovery deadlines, retries, and
  multi-device policy are unchanged. The initial native probe and post-credential
  retry remain distinct because they have different credential-loading behavior.
- [x] 13. Separate read-only pending-journal matching (`9d9f210bc`) from update
  reconciliation, and construct stage IDs once per search. Adoption remains
  under the existing device lock. All 41 update tests pass, including ambiguous
  journal rejection without mutation. Publication, attendance, reboot, recovery,
  and rollback guarantees remain intact.
- [x] 14. Remove discarded distribution hashes (`f7fceafad`). Layout checks
  enumerate names without hashing every payload; ZIP and Downloader receipt
  comparisons still hash payload bytes. Independent artwork, manifest, and
  manager checks remain. All 13 distribution/workflow tests pass, including
  same-name payload corruption rejection. No additional database refactor was
  justified beyond the unused APIs removed in item 2.

## Validation boundary

The final branch passes the 308-test no-default-feature app suite and app Clippy
with warnings denied. Focused Rust LSP diagnostics are clear. The final Python
rerun passes 48 update/compatibility tests and 56 distribution, architecture, CI,
and registry tests (plus 17 subtests). Rust formatting, build contracts, 17
pre-commit fixtures, repository and launcher contracts, and focused tests listed
above were also checked during implementation. Logs are local artifacts, not
committed evidence.

The final UI-feature suite completes with **1,352 passed, 10 failed, and one
ignored**. The untouched base completes with **1,353 passed, the same 10 failed,
and one ignored**. All ten failures report `The Slint platform was initialized
in another thread`; they are confirmed pre-existing process-global test
isolation failures; all ten pass individually on the final branch. The exclusive
software-renderer raster test passes when
run separately with `--ignored --exact`. The earlier storage blocker is resolved;
no shared caches were deleted.

Broad ARM, full CI, visual inspection, and physical device qualification have
not been run. No push, deployment, or Desktop modification was performed. The
Rust LSP and Slint skills guided symbol checks and preserved feature/build
boundaries; software-renderer tests are not a visual or device qualification.

Deleted files remain recoverable from the base commit above. The original main
checkout is unchanged by implementation work.
