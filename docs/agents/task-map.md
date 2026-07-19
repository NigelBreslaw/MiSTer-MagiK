# Task routing map

Start with the narrowest matching row. `Full check` is host-only unless the
validation column explicitly calls for ARM, GUI, or an attended device.

| Task | Start here | Canonical docs | Fast check | Full check | Extra validation | Normally irrelevant |
|---|---|---|---|---|---|---|
| Launcher navigation | `apps/mister/src/launcher.rs` | `docs/architecture.md` | `scripts/dev-rust test` | `scripts/validate paths apps/mister/src/launcher.rs` | UI/device only for visual or controller behavior | `history/`, `apps/desktop/`, `kernel/` |
| Launcher lifecycle | `apps/mister/src/ui_runner/launcher_lifecycle.rs` | `docs/architecture.md` | `scripts/dev-rust test` | `scripts/validate paths apps/mister/src/ui_runner` | Attended device for Main handoff/recovery | `documentation/`, `fpga/` unless display-related |
| Catalog and discovery | `crates/catalog/src/` | `docs/catalog.md` | `cargo test --manifest-path crates/catalog/Cargo.toml` | `scripts/validate paths crates/catalog` | Device acceptance only for real-library behavior | `apps/desktop/vendor/`, `history/` |
| Framebuffer/presentation | `mister/platform/runtime/src/framebuffer/` | `docs/architecture.md`, `docs/device.md` | `cargo test --manifest-path mister/platform/runtime/Cargo.toml` | `scripts/validate paths mister/platform/runtime` | ARM and attended HDMI proof for scan-out claims | `documentation/`, catalog media |
| Input/controllers | `crates/magik-core/src/input_state.rs`, `apps/mister/src/input.rs` | `docs/device.md` | `scripts/dev-rust test` | `scripts/validate paths crates/magik-core apps/mister/src/input.rs` | Attended controller test for Linux mappings | `apps/desktop/`, MiSTer platform rendering |
| Media/previews | `apps/mister/src/media_update.rs`, `crates/catalog/src/preview_worker.rs` | `docs/catalog.md` | `scripts/dev-rust test` | `scripts/validate paths apps/mister/src/media_update.rs crates/catalog/src/preview_worker.rs` | Use private submodule tools for generated packs | `reference/`, raw device cache directories |
| Slint UI | `apps/mister/ui/launcher.slint` | `apps/mister/BUILD.md` | `cargo check --manifest-path apps/mister/Cargo.toml --bin mister-magik-fb --no-default-features --features ui` | `scripts/validate paths apps/mister/ui` | Escalated `apps/mister/build-arm.sh --check --ui-scope launcher` before ARM/device claims; visual validation for layout | `history/`, host tooling |
| Host MiSTer tool | `mister/tools/host/src/main.rs` | `docs/device.md` | `cargo test --manifest-path mister/tools/host/Cargo.toml` | `scripts/validate paths mister/tools/host` | No device unless command execution is under test | `apps/desktop/`, `fpga/` |
| MagiK agent | `mister/tools/agent/src/main.rs` | `docs/magik-agent.md` | `cargo test --manifest-path mister/tools/agent/Cargo.toml` | `scripts/validate paths mister/tools/agent` | ARM/device for Linux-only operations | `documentation/`, launcher UI |
| Desktop UI | `apps/desktop/src/main.rs`, `apps/desktop/ui/main.slint` | `apps/desktop/AGENTS.md` | `cargo test --manifest-path apps/desktop/Cargo.toml` | `scripts/validate paths apps/desktop` | Slint MCP visual check for UI changes | MiSTer kernel/FPGA and device launcher internals |
| Documentation | `documentation/src/content/docs/` | `documentation/src/content/docs/contributing/` | `corepack pnpm --dir documentation run build` | `scripts/validate paths documentation` | None | Rust targets, device |
| Packaging/releases | `scripts/package-distribution.sh` | `docs/releases.md` | `scripts/test-host-tools.sh --full` | `scripts/validate paths scripts/package-distribution.sh` | Release workflow only after host checks | UI runtime internals |
| Kernel scanout | `mister/platform/kernel/scanout-slots/` | `docs/kernel-scanout-plugin-assurance.md` | `scripts/checks/check-scanout-slots-contract.sh` | `scripts/validate full-host` | Kernel build and attended device qualification | Catalog, desktop |
| FPGA latch | `mister/platform/fpga/menu-vblank-latch/` | `docs/fpga-latch-release.md` | `python3 scripts/checks/check-fpga-latch-coverage.py --help` | `scripts/validate full-host` | Quartus/device signoff | Catalog, documentation |

For dated evidence, search explicitly with `rg --no-ignore history/`. For
unknown work, run `scripts/validate paths PATH... --print-plan` before invoking
checks.
