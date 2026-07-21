# Script Deletion Ledger

This ledger records script interfaces removed from the current repository.
Git retains their implementation history; retired implementations are not
archived elsewhere in the working tree.

## Catalog V2 tombstones

Deleted on 2026-07-20 after static inspection showed that every executable
unconditionally exited with a retirement error before reaching its retained
implementation:

- `bench-library.sh`
- `build-local-library-db.sh`
- `device-catalog-destruction.sh`
- `device-catalog-drift-acceptance.sh`
- `device-fs-fault-reset.sh`
- `device-launcher-catalog-publication-regression.sh`
- `device-library-change-flow.sh`
- `device-prepared-collections-acceptance.sh`
- `device-warm-catalog-summary-missing-preview-regression.sh`
- `profile-library-io.sh`
- `profile-warm-catalog-start.sh`

Together these files contained 3,292 lines, almost all unreachable retired V2
implementation. Current V3 catalog acceptance and profiling interfaces remain
under `scripts/`.

## Pass-through adapters

Deleted on 2026-07-20 after updating callers to use their canonical interfaces:

- `install-slint-boot.sh` → `magik-mode.sh dev`
- `restore-stock-boot.sh` → `magik-mode.sh stock`
- `gate-cold-preview-systems.sh` → `profile-cold-preview-systems.sh --require-pass`
- `gate-cold-turbo-preview.sh` → `profile-cold-turbo-preview.sh --require-pass`

## Unowned device workflows

Deleted on 2026-07-21 after explicit capability review found no current caller
or policy owner. Their useful safety requirements are already owned by the
typed deployment workflow; the remaining implementations were standalone
diagnostics, duplicated polling and cleanup, flag-driven profiling, or unsafe
fault experiments:

- `fpga-vblank-latch-one-shot.sh`
- `profile-scene-report.sh`
- `device-arcade-filter-navigation.sh`
- `mister-early-dhcpcd-service.sh`
- `profile-screensaver-preview.sh`
- `audit-idle-cpu.sh`
- `qualify-fpga-latch-release.sh`
- `capture-launcher-home-pan-video.sh`
- `profile-analytics-overhead.sh`
- `device-catalog-resume-acceptance.sh`
- `device-resource-exhaustion.sh`
- `switch-ui.sh`
- `mister-asset-diagnostics.sh`

The deleted scripts are not compatibility interfaces. Git history retains
their implementations if a future requirement supplies a concrete owner and
acceptance test.

## Compatibility implementations

`agent-cli` owns the public typed intents for affected checks, complete host
verification, Rust checks, doctor, host-tool checks, host release checks, and
Apple-container ARM compilation. The five former orchestration entrypoints were
deleted after their retained capabilities moved into the typed registry.
# Deployment takeover

- `scripts/deploy-rust.sh` — removed; runtime deployment is owned by `scripts/agent deliver` and internal typed recipes.
- `scripts/deploy-platform.sh` — removed; qualified platform installation is owned by `scripts/agent deliver`.

## Typed ARM build takeover

Reviewed on 2026-07-21. `apps/mister/build-arm.sh`,
`apps/mister/build-arm64-apple-container.sh`, and
`scripts/build-mister-agent.sh` selected profiles/backends, assembled commands,
and copied artifacts. That orchestration and its safety invariants are now
owned by `agent-cli/src/build.rs`; callers use fixed typed intents. The wrappers,
their resource library, no-op regression wrapper, and wrapper-specific tests
were deleted rather than retained as compatibility paths.

## Transactional delivery takeover

Reviewed on 2026-07-21. `scripts/scanout-slots-one-shot.sh` built and installed a
temporary module and diagnostics binary, mutated the live device, then attempted
ad-hoc restoration. Its owned readiness checks now run in the typed delivery
smoke phase; the standalone mutation workflow was deleted. Source builders and
pure contract checks remain because CI still owns them.
