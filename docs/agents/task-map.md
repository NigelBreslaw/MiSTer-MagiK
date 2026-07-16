# Task routing map

Start with the narrowest matching row. `Full check` is host-only unless the
validation column explicitly calls for ARM, GUI, or an attended device.

| Task | Start here | Canonical docs | Fast check | Full check | Extra validation | Normally irrelevant |
|---|---|---|---|---|---|---|
| Launcher navigation | `magik-gui/src/launcher.rs` | `docs/architecture.md` | `scripts/dev-rust test` | `scripts/validate paths magik-gui/src/launcher.rs` | UI/device only for visual or controller behavior | `history/`, `desktop/`, `kernel/` |
| Launcher lifecycle | `magik-gui/src/ui_runner/launcher_lifecycle.rs` | `docs/architecture.md` | `scripts/dev-rust test` | `scripts/validate paths magik-gui/src/ui_runner` | Attended device for Main handoff/recovery | `documentation/`, `fpga/` unless display-related |
| Catalog and discovery | `magik-gui/catalog/src/` | `docs/catalog.md` | `cargo test --manifest-path magik-gui/catalog/Cargo.toml` | `scripts/validate paths magik-gui/catalog` | Device acceptance only for real-library behavior | `desktop/vendor/`, `history/` |
| Framebuffer/presentation | `magik-gui/src/framebuffer/` | `docs/architecture.md`, `docs/device.md` | `scripts/dev-rust test` | `scripts/validate paths magik-gui/src/framebuffer` | ARM and attended HDMI proof for scan-out claims | `documentation/`, catalog media |
| Input/controllers | `magik-gui/src/input_state.rs`, `input.rs` | `docs/device.md` | `scripts/dev-rust test` | `scripts/validate paths magik-gui/src/input_state.rs magik-gui/src/input.rs` | Attended controller test for Linux mappings | `desktop/`, `fpga/` |
| Media/previews | `magik-gui/src/media_update.rs`, `catalog/src/preview_worker.rs` | `docs/catalog.md` | `scripts/dev-rust test` | `scripts/validate paths magik-gui/src/media_update.rs magik-gui/catalog/src/preview_worker.rs` | Use private submodule tools for generated packs | `reference/`, raw device cache directories |
| Slint UI | `magik-gui/ui/launcher.slint` | `magik-gui/BUILD.md` | `cargo check --manifest-path magik-gui/Cargo.toml --bin mister-magik-fb --no-default-features --features ui` | `scripts/validate paths magik-gui/ui` | Escalated `magik-gui/build-arm.sh --check --ui-scope launcher` before ARM/device claims; visual validation for layout | `history/`, host tooling |
| Host MiSTer tool | `tools/mister/src/main.rs` | `docs/device.md` | `cargo test --manifest-path tools/mister/Cargo.toml` | `scripts/validate paths tools/mister` | No device unless command execution is under test | `desktop/`, `fpga/` |
| MagiK agent | `tools/magik-agent/src/main.rs` | `docs/magik-agent.md` | `cargo test --manifest-path tools/magik-agent/Cargo.toml` | `scripts/validate paths tools/magik-agent` | ARM/device for Linux-only operations | `documentation/`, launcher UI |
| Desktop UI | `desktop/src/main.rs`, `desktop/ui/main.slint` | `desktop/AGENTS.md` | `cargo test --manifest-path desktop/Cargo.toml` | `scripts/validate paths desktop` | Slint MCP visual check for UI changes | `kernel/`, `fpga/`, device launcher internals |
| Documentation | `documentation/src/content/docs/` | `documentation/src/content/docs/contributing/` | `corepack pnpm --dir documentation run build` | `scripts/validate paths documentation` | None | Rust targets, device |
| Packaging/releases | `scripts/package-distribution.sh` | `docs/releases.md` | `scripts/test-host-tools.sh --full` | `scripts/validate paths scripts/package-distribution.sh` | Release workflow only after host checks | UI runtime internals |
| Kernel scanout | `kernel/scanout-slots/` | `docs/kernel-scanout-plugin-assurance.md` | `scripts/checks/check-scanout-slots-contract.sh` | `scripts/validate full-host` | Kernel build and attended device qualification | Catalog, desktop |
| FPGA latch | `fpga/menu-vblank-latch/` | `docs/fpga-latch-release.md` | `python3 scripts/checks/check-fpga-latch-coverage.py --help` | `scripts/validate full-host` | Quartus/device signoff | Catalog, documentation |

For dated evidence, search explicitly with `rg --no-ignore history/`. For
unknown work, run `scripts/validate paths PATH... --print-plan` before invoking
checks.
