# Task routing map

Start with the narrowest matching row. `Full check` is host-only unless the
validation column explicitly calls for ARM, GUI, or an attended device.

| Task | Start here | Canonical docs | Fast check | Full check | Extra validation | Normally irrelevant |
|---|---|---|---|---|---|---|
| Launcher navigation | `apps/mister/src/launcher.rs` | `docs/architecture.md` | `scripts/agent check` | `scripts/agent verify` | UI/device only for visual or controller behavior | `history/`, `apps/desktop/`, `kernel/` |
| Launcher lifecycle | `apps/mister/src/ui_runner/launcher_lifecycle.rs` | `docs/architecture.md` | `scripts/agent check` | `scripts/agent verify` | ARM validation is selected automatically; attended device only for handoff/recovery | `documentation/`, `fpga/` unless display-related |
| Catalog and discovery | `crates/catalog/src/` | `docs/catalog.md` | `scripts/agent check` | `scripts/agent verify` | `scripts/agent benchmark` for real-library behavior | `apps/desktop/vendor/`, `history/` |
| Framebuffer/presentation | `mister/platform/runtime/src/framebuffer/` | `docs/architecture.md`, `docs/device.md` | `scripts/agent check` | `scripts/agent verify` | ARM and attended HDMI proof for scan-out claims | `documentation/`, catalog media |
| Input/controllers | `crates/magik-core/src/input_state.rs`, `apps/mister/src/input.rs` | `docs/device.md` | `scripts/agent check` | `scripts/agent verify` | Attended controller test for Linux mappings | `apps/desktop/`, MiSTer platform rendering |
| Media/previews | `apps/mister/src/media_update.rs`, `crates/catalog/src/preview_worker.rs` | `docs/catalog.md` | `scripts/agent check` | `scripts/agent verify` | Use private submodule tools for generated packs | `reference/`, raw device cache directories |
| Slint UI | `apps/mister/ui/launcher.slint` | `apps/mister/BUILD.md` | `scripts/agent check` | `scripts/agent verify` | ARM validation is selected automatically; visual validation for layout | `history/`, host tooling |
| Host MiSTer tool | `mister/tools/host/src/main.rs` | `docs/device.md` | `scripts/agent check` | `scripts/agent verify` | No device unless command execution is under test | `apps/desktop/`, `fpga/` |
| MagiK agent | `mister/tools/agent/src/main.rs` | `docs/magik-agent.md` | `scripts/agent check` | `scripts/agent verify` | ARM/device for Linux-only operations | `documentation/`, launcher UI |
| Installer manager | `mister/tools/manager/src/main.rs` | `docs/installer.md` | `scripts/agent check` | `scripts/agent verify` | ARM/device only after host fixtures pass | launcher rendering, catalog policy |
| Rust semantic tooling | `.codex/config.toml`, `.lspi/config.toml`, `scripts/rust-analyzer`, `apps/mister/rust-toolchain.toml` | `AGENTS.md` | `scripts/agent check` | `scripts/agent deliver` | Doctor contract and read-only MCP smoke test | Device communication, runtime deployment |
| Desktop UI | `apps/desktop/src/main.rs`, `apps/desktop/ui/main.slint` | `apps/desktop/AGENTS.md` | `scripts/agent check` | `scripts/agent verify` | Slint MCP visual check for UI changes | MiSTer kernel/FPGA and device launcher internals |
| Documentation | `documentation/src/content/docs/` | `documentation/src/content/docs/contributing/` | `scripts/agent check` | `scripts/agent verify` | None | Rust targets, device |
| Packaging/releases | `scripts/package-distribution.sh` | `docs/releases.md` | `scripts/agent check` | `scripts/agent verify`; operator gate: `scripts/agent release qualify` | Release workflow only after host checks | UI runtime internals |
| Kernel scanout | `mister/platform/kernel/scanout-slots/` | `docs/kernel-scanout-plugin-assurance.md` | `scripts/agent check` | `scripts/agent verify` | Kernel build and attended device qualification | Catalog, desktop |
| FPGA latch | `mister/platform/fpga/menu-vblank-latch/` | `docs/fpga-latch-release.md` | `scripts/agent check` | `scripts/agent verify` reports required GitHub Actions RBF build | Quartus/device signoff | Catalog, documentation |

For dated evidence, search explicitly with `rg --no-ignore history/`. For
unknown work, begin the task before editing and run `scripts/agent plan` before invoking
checks.

The normal feature loop is `scripts/agent task begin`, edit,
`scripts/agent check`, and `scripts/agent commit -m MESSAGE`. Commit performs
full verification of the exact staged tree. Use standalone `verify` only when
assurance is needed without creating a commit.
Runtime or platform changes then use `scripts/agent deliver`. Delivery uses the
exact clean local app and Main commits without consulting task records or
requiring publication. Every delivery uses the complete platform transaction
because the development manifest binds `mister-magik-fb`, Main, the kernel
module, and the FPGA latch into one coherent set.
# Deployment

“Build and deploy” maps to `scripts/agent commit -m MESSAGE` followed by
`scripts/agent deliver`. The CLI owns platform qualification and device
transactions, and records
progress and evidence. Performance, diagnosis, and attended acceptance use the
typed `benchmark`, `diagnose`, and `release qualify` state machines.
