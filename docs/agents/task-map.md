# Task routing map

Start with the narrowest matching row. `Full check` is host-only unless the
validation column explicitly calls for ARM, GUI, or an attended device.

| Task | Start here | Canonical docs | Fast check | Full check | Extra validation | Normally irrelevant |
|---|---|---|---|---|---|---|
| Launcher navigation | `apps/mister/src/launcher.rs` | `docs/architecture.md` | `scripts/agent check --paths apps/mister/src/launcher.rs` | `scripts/agent verify --paths apps/mister/src/launcher.rs` | UI/device only for visual or controller behavior | `history/`, `apps/desktop/`, `kernel/` |
| Launcher lifecycle | `apps/mister/src/ui_runner/launcher_lifecycle.rs` | `docs/architecture.md` | `scripts/agent check --paths apps/mister/src/ui_runner` | `scripts/agent verify --paths apps/mister/src/ui_runner` | `scripts/agent arm check-launcher`; attended device only for handoff/recovery | `documentation/`, `fpga/` unless display-related |
| Catalog and discovery | `crates/catalog/src/` | `docs/catalog.md` | `scripts/agent check --paths crates/catalog` | `scripts/agent verify --paths crates/catalog` | Device acceptance only for real-library behavior | `apps/desktop/vendor/`, `history/` |
| Framebuffer/presentation | `mister/platform/runtime/src/framebuffer/` | `docs/architecture.md`, `docs/device.md` | `scripts/agent check --paths mister/platform/runtime` | `scripts/agent verify --paths mister/platform/runtime` | ARM and attended HDMI proof for scan-out claims | `documentation/`, catalog media |
| Input/controllers | `crates/magik-core/src/input_state.rs`, `apps/mister/src/input.rs` | `docs/device.md` | `scripts/agent check --paths crates/magik-core apps/mister/src/input.rs` | `scripts/agent verify --paths crates/magik-core apps/mister/src/input.rs` | Attended controller test for Linux mappings | `apps/desktop/`, MiSTer platform rendering |
| Media/previews | `apps/mister/src/media_update.rs`, `crates/catalog/src/preview_worker.rs` | `docs/catalog.md` | `scripts/agent check --paths apps/mister/src/media_update.rs crates/catalog/src/preview_worker.rs` | `scripts/agent verify --paths apps/mister/src/media_update.rs crates/catalog/src/preview_worker.rs` | Use private submodule tools for generated packs | `reference/`, raw device cache directories |
| Slint UI | `apps/mister/ui/launcher.slint` | `apps/mister/BUILD.md` | `scripts/agent check --paths apps/mister/ui` | `scripts/agent verify --paths apps/mister/ui` | `scripts/agent arm check-launcher`; visual validation for layout | `history/`, host tooling |
| Host MiSTer tool | `mister/tools/host/src/main.rs` | `docs/device.md` | `scripts/agent check --paths mister/tools/host` | `scripts/agent verify --paths mister/tools/host` | No device unless command execution is under test | `apps/desktop/`, `fpga/` |
| MagiK agent | `mister/tools/agent/src/main.rs` | `docs/magik-agent.md` | `scripts/agent check --paths mister/tools/agent` | `scripts/agent verify --paths mister/tools/agent` | ARM/device for Linux-only operations | `documentation/`, launcher UI |
| Desktop UI | `apps/desktop/src/main.rs`, `apps/desktop/ui/main.slint` | `apps/desktop/AGENTS.md` | `scripts/agent check --paths apps/desktop` | `scripts/agent verify --paths apps/desktop` | Slint MCP visual check for UI changes | MiSTer kernel/FPGA and device launcher internals |
| Documentation | `documentation/src/content/docs/` | `documentation/src/content/docs/contributing/` | `scripts/agent check --paths documentation` | `scripts/agent verify --paths documentation` | None | Rust targets, device |
| Packaging/releases | `scripts/package-distribution.sh` | `docs/releases.md` | `scripts/agent check --paths scripts/package-distribution.sh` | `scripts/agent release host` | Release workflow only after host checks | UI runtime internals |
| Kernel scanout | `mister/platform/kernel/scanout-slots/` | `docs/kernel-scanout-plugin-assurance.md` | `scripts/agent check --paths mister/platform/kernel/scanout-slots` | `scripts/agent verify full-host` | Kernel build and attended device qualification | Catalog, desktop |
| FPGA latch | `mister/platform/fpga/menu-vblank-latch/` | `docs/fpga-latch-release.md` | `scripts/agent check --paths mister/platform/fpga/menu-vblank-latch` | `scripts/agent verify full-host` | Quartus/device signoff | Catalog, documentation |

For dated evidence, search explicitly with `rg --no-ignore history/`. For
unknown work, run `scripts/agent plan --paths PATH...` before invoking
checks.
