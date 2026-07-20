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

## Awaiting capability review

These active scripts have no current repository callers. Absence of a caller
does not prove absence of an undocumented human workflow, so each requires an
explicit keep-or-delete decision before migration:

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
