# Simplification implementation checklist

Base: `3b8d2c3f2d38278b9319fadcddaa40ccb477901d` (`main`, including #151/#152).
Branch: `nigel/project-simplification`. Desktop is retained unchanged.

This is a **partial implementation**, not a completed project-wide cleanup.
Each entry below corresponds to the planned logical commit boundary.

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
- [ ] 5. Retire unfinished effects and experimental transition variants.
  **Unchanged:** auto-review rejected mechanical removal across preview
  rendering feature gates because a mistaken boundary could remove production
  behavior. Requires approval and individually reviewed patches with production
  fade and benchmark feature validation.
- [x] 6. Remove the orphan Jersey10 license sidecar (`02ce5ef57`). Other font
  notices are retained.
- [x] 7. Remove the unused Rust diagnostics mirror (`5d1bfc9fa`). Remove the
  isolated crate, Rust-only generator output, and obsolete CI/cache/input
  wiring. Preserve schema validation, opcodes, RTL, and generated SystemVerilog.
  Protocol checks, existing FPGA simulations, cache tests, and 20 component-ID
  tests pass. Component cache identity changes; no hardware was deployed.
- [ ] 8. Remove archived raw history with provenance.
  **Unchanged:** auto-review rejected suffix-based bulk deletion because raw
  files may retain meaningful qualification evidence. Review each candidate,
  retain referenced summaries, and record original paths, source commit, and
  hashes before an approved deletion. Do not rewrite Git history.
- [x] 9. Simplify architecture reporting (`80746a589`). Remove the retired
  SQLite hotspot and unreliable brace-counted function-size metric; filter
  archived/generated paths from concentration totals. Version the JSON schema
  as v2. Reporter and CLI tests pass.
- [ ] 10. Simplify launcher configuration and diagnostic state.
  **Partial:** benchmark defaults now use the existing capture parser
  (`cd9c2d6c3`); all six benchmark tests pass. No diagnostic state was removed.
  Larger changes remain subject to demonstrated redundancy.
- [ ] 11. Simplify launcher orchestration through existing owners.
  **Outstanding:** the #151/#152 rendering pipeline is unchanged. Do not split
  files solely to reduce line counts. Any frame-ordering change needs sequence
  tests and physical device evidence.
- [ ] 12. Simplify discovery and connection branches.
  **Partial:** consolidate native connection event reporting and explicit
  authentication-error handling (`bafc12a9b`). All 21 discovery/compatibility
  tests pass. Discovery deadlines, retries, and multi-device policy are unchanged.
- [ ] 13. Simplify transactional update orchestration.
  **Outstanding:** no changes to locks, publication journals, reconciliation,
  attendance requirements, or rollback. Require focused transition tests before
  changing repeated decisions.
- [ ] 14. Simplify database/distribution duplication.
  **Unchanged:** auto-review rejected removing payload hashing from the layout
  inventory because of the risk to verification guarantees. Before approval,
  demonstrate that hashes discarded during name enumeration are redundant with
  the retained ZIP/Downloader receipt checks. No database refactor is included.

## Validation boundary

The completed changes pass the 308-test no-default-feature app suite, app
Clippy with warnings denied, Rust formatting, focused Rust LSP diagnostics,
registry checks, architecture/CLI tests, 17 pre-commit fixtures, repository and
launcher contracts, and the focused tests listed above. Logs are local artifacts,
not committed evidence. Broad ARM, full CI, and physical device qualification
have not been run. No push, deployment, or Desktop modification was performed.

Deleted files remain recoverable from the base commit above. The original main
checkout is unchanged by implementation work.
