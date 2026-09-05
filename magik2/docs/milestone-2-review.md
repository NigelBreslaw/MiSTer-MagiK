# Shared Mini-MagiK milestone review

Reviewed and corrected on 5 September 2026. The default development workflow has
no remaining blocking finding from this review. This is bounded source and
device evidence, not a claim that every failure mode has been exercised.

## Correction plan and outcome

1. Select development data paths at their shared source of authority.
2. Expose actual application paths in metrics and assert them in smoke checks.
3. Deploy once and verify the running artifact and paths without another restart.
4. Review shared lifecycle, capability selection, measurements and test scope;
   correct concrete findings in separate commits.

All four steps are complete. No transfer or Mini-MagiK performance runs were
repeated, and no performance tuning was attempted.

## Findings and corrections

| Priority | Finding | Correction |
|---|---|---|
| P1 | A separate executable slot did not isolate data. Executable-directory inference treated `mister-magik2/magik` as production. | The real app's `magik2` feature selects catalog's `development-layout`. Both current-layout APIs use the same decision. Ordinary builds retain their existing behavior. |
| P1 | Real-app workflows could retain an agent lacking the Main input launch support they required, causing the input-unavailable banner. | The service advertises `main-input-proxy`; real-app deploy, watch and scenario connections request it. Missing support triggers automatic replacement, without version equality. Mini does not require it. |
| P2 | A completed earlier idle window could be accepted when a new measurement request was not consumed. | Reject windows whose start does not follow the previously completed window. A regression test supplies unchanged evidence. |
| P2 | A failed scenario could continue into later measurements, wasting time and obscuring the initial failure. | CLI scenario runs stop at the first failure. The shared session and profile-only selection avoid repeated application launches. |
| P2 | Runtime path isolation was absent from acceptance evidence. | Metrics expose the paths actually captured by the launcher. Smoke rejects missing, production or parent-traversal paths. CI includes the focused central-layout regression. |

The path assertion checks reported paths; it is not a filesystem sandbox or a
remote symlink audit. Explicit catalog path overrides remain supported. The
normal development defaults are now correct, and smoke reports overrides that
escape the development tree.

## Scope and cleanliness assessment

The shared design is appropriate for this milestone: both applications consume
one observation/measurement/profile crate and one native service, host client
and pytest scenario system. Real-app integration remains feature-gated. The
layout correction adds a compile feature to the existing path authority rather
than another manifest, protocol version or executable-directory exception.

The review covered named artifact slots, staged versus running hashes, Main
handoff and input context, process ownership and session cleanup, preview cache
invalidation, measurement freshness, build inputs and ownership/CI coverage.
Mini and real app use the same preview/profile/measurement modules; the real
preview publishes committed frames. Build handling retains embedded-font input
fingerprints, the real app's intended build profile and minimal FFmpeg setup.
The new tooling does not invoke the legacy agent workflow.

The two applications deliberately retain different workloads: Mini provides a
controlled motion experiment; real MagiK currently measures launcher idle. This
keeps the common mechanism small, but does not yet establish equivalent app
performance. No additional abstraction is needed for this milestone.

## Verification

- Central layout: four focused Cargo tests passed with
  `--features builder,development-layout device_layout`. The first attempt
  without `builder` exposed unrelated existing test compilation assumptions;
  CI uses the complete feature selection.
- Host changes: 51 tests passed in 0.23 seconds before the final freshness/path
  additions. The affected scenario suite then passed all nine tests in 0.02
  seconds, including stale evidence and parent traversal.
- Native Main input context: two focused tests passed.
- Manifest synchronization: no dependency or lockfile change required.
- Corrected ARM app build and deployment passed. The service upgraded
  automatically for the missing input capability. The running artifact hash
  matched its metrics evidence.
- Read-only device verification confirmed every path below without another
  app restart. Completed transfer, motion and profiling evidence was retained.

| Runtime value | Observed path |
|---|---|
| Data root | `/media/fat/mister-magik-dev` |
| Main | `/media/fat/MiSTer_MagiKDev` |
| Settings | `/media/fat/mister-magik-dev/settings.json` |
| Controllers | `/media/fat/mister-magik-dev/controllers.json` |
| Catalog | `/media/fat/mister-magik-dev/catalog-fast-v1` |
| Library | `/media/fat/mister-magik-dev/library.sqlite3` |
| User state | `/media/fat/mister-magik-dev/user-state.sqlite3` |
| Assets | `/media/fat/mister-magik-dev/assets` |

Local evidence bundles under `build/magik2-results/`:
`20260905T160815Z-e1d086d0c5ce` (deployment) and
`20260905T161135Z-c113eb76215f` (running identity and paths).
Raw artifacts remain outside Git.

Rust LSP rejected the linked worktree because its configured allowed root is
the primary checkout. Source inspection, focused Cargo tests and the ARM build
provided the available validation; no broad local assurance was run.

## Limits and interpretation

Earlier real-app smoke, idle, profile and stream runs used production data
paths. They demonstrate tooling function, but must not be presented as a
corrected development-layout performance baseline. Earlier production runtime
data changes cannot be enumerated confidently without a pre-run snapshot; no
speculative rollback was attempted. Production executable/platform files were
not replaced by the separate app deployment.

Real idle windows had zero redraws, and the profile contained only 13 samples.
That validates the pipeline, not animation performance or useful hotspot
comparisons. Physical controller button input was not automated; the earlier
smoke assertion checked the application's input-health indication.

ROM sources, the official asset download URL and Main's `/tmp/mister-magik`
control endpoints are intentionally shared. The download URL is an asset
source, not the deployment destination; downloaded assets now default to Dev.
Prebuilt real-app overrides must be built with `magik2`, just as automatic
builds are. Explicit user path overrides are not silently rewritten.

The [milestone record](milestone-2.md) retains previous measurements and their
interpretation. No further hardware tests are required to close these findings.
