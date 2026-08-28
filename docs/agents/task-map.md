# Task routing map

Use this map only when the owning subsystem, canonical document, or exceptional
assurance is unclear. The nearest `AGENTS.md` governs the work.

For Rust, use `$magik-rust-lsp`; for Slint behavior, use the Slint MCP. Normal
assurance belongs to pre-commit, pre-push, and CI. Run `scripts/agent plan` to
preview affected assurance. The last column lists only work beyond those
defaults.

| Area | Start here | Canonical docs | Extra assurance |
|---|---|---|---|
| Architecture debt or executable boundaries | Affected boundary, adapter, or contract | `docs/agents/architecture-debt-ledger.md`, `docs/architecture.md` | Device only for changed runtime behavior |
| Launcher behavior and lifecycle | `apps/mister/src/`, then its nearest `AGENTS.md` | `docs/architecture.md` | Attended device checks for handoff, recovery, or controller behavior |
| Launcher Slint UI | `apps/mister/ui/api.slint`, then the matching variant and view | `docs/architecture.md`, `apps/mister/BUILD.md` | Visual matrix; attended capture only for physical HDMI/CRT claims |
| RGB565 scenes and particles | `crates/framebuffer-scenes/`, `crates/particles/`, `apps/framebuffer-scene-lab/` | `docs/startup-particles.md` | Preview or attended device lab for visual or performance claims |
| Catalog and media | `crates/catalog/src/`, `apps/mister/src/media_update.rs` | `docs/catalog.md` | Benchmark real-library behavior; use private tools for generated packs |
| Platform framebuffer | `mister/platform/runtime/src/framebuffer/` | `docs/architecture.md`, `docs/device.md` | Attended HDMI proof for scan-out claims |
| Host workflow tool | `agent-cli/src/` | `docs/device.md` | Device only when command execution is under test |
| Device agent | `mister/tools/agent/src/main.rs` | `docs/magik-agent.md` | ARM/device checks for Linux-only operations |
| Installer manager | `mister/tools/manager/src/main.rs` | `docs/installer.md` | ARM/device only after host fixtures pass |
| Desktop companion | `apps/desktop/AGENTS.md` | That file | Visual verification for UI changes |
| Public documentation | `documentation/src/content/docs/` | `documentation/src/content/docs/contributing/` | None |
| Packaging and releases | `scripts/package-distribution.sh` | `docs/releases.md` | Explicit operator request for `scripts/agent release qualify` |
| Kernel scanout | `mister/platform/kernel/scanout-slots/` | `docs/kernel-scanout-plugin-assurance.md` | Kernel build and attended device qualification |
| FPGA latch | `mister/platform/fpga/menu-vblank-latch/` | `docs/fpga-latch-release.md` | Typed FPGA signoff and attended device qualification |
| Generated, private, or device-owned files | Affected path | `docs/agents/file-authority.md` | Follow the owning regeneration or deployment route |

Search dated evidence explicitly with `rg --no-ignore history/`.
