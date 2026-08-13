# Task routing map

Start with the narrowest matching row. Rust edit-time feedback uses the
`$magik-rust-lsp` skill. Full automated assurance runs when pushing and in CI;
the extra-assurance column identifies attended or specialized work beyond it.

| Task | Start here | Canonical docs | Edit-time feedback | Automated assurance | Extra assurance | Normally irrelevant |
|---|---|---|---|---|---|---|
| P1 executable-boundary enforcement | `mister/platform/runtime/src/lib.rs`, `mister/platform/contracts/platform-v3.schema.toml`; consult the ledger before shared app or agent-cli seams | `docs/agents/architecture-debt-ledger.md`, `docs/architecture.md` | `$magik-rust-lsp` | Pre-push and CI | Device only when a later commit explicitly changes an attended operation | Launcher frame-loop decomposition, Slint presenter migration |
| P1 launcher decomposition | `apps/mister/src/ui_runner/launcher_loop.rs`; start with frame-phase characterization | `docs/agents/architecture-debt-ledger.md`, `docs/architecture.md` | `$magik-rust-lsp` | Pre-push and CI select launcher/ARM assurance | Deterministic visual matrix before visual or presentation claims | Platform-v3 contract, config schema, agent host internals |
| P1 typed configuration | `apps/mister/config/runtime-environment.toml`, then the selected process entry; consult the ledger before `fs_fault.rs` or layout work | `docs/agents/architecture-debt-ledger.md`, `docs/reference/mister-runtime-environment.md` | `$magik-rust-lsp` | Pre-push and CI | Visual matrix and Slint MCP only for the later typed-UI lane | Main transport implementation, launcher frame-phase extraction |
| Launcher navigation control | `apps/mister/src/launcher.rs`, `apps/mister/src/launcher_runtime/navigation_transition.rs` | `docs/architecture.md` | `$magik-rust-lsp` | Pre-push and CI | UI/device only for visual or controller behavior | `history/`, `apps/desktop/`, `kernel/` |
| Portable RGB565 scene or navigation rasterizer | `crates/framebuffer-scenes/`, `apps/framebuffer-scene-lab/` | `docs/architecture.md`, `docs/startup-particles.md` | `$magik-rust-lsp` | Pre-push and CI select shared-scene, focused-lab, and consumer checks | macOS preview or attended fixed-HDMI scene lab for visual/performance claims | Slint product state, Main/latch implementation, catalog loading |
| Launcher lifecycle | `apps/mister/src/launcher_runtime/lifecycle.rs`; UI-runner adapters in `apps/mister/src/ui_runner/launcher_bridge.rs`, `launcher_scheduler.rs`, and `*_session.rs` | `docs/architecture.md` | `$magik-rust-lsp` | Pre-push and CI select ARM validation | Attended device only for handoff/recovery | `documentation/`, `fpga/` unless display-related |
| Catalog and discovery | `crates/catalog/src/` | `docs/catalog.md` | `$magik-rust-lsp` | Pre-push and CI | `scripts/agent benchmark` for real-library behavior | `apps/desktop/vendor/`, `history/` |
| Framebuffer/presentation | `mister/platform/runtime/src/framebuffer/` | `docs/architecture.md`, `docs/device.md` | `$magik-rust-lsp` | Pre-push and CI select ARM validation | Attended HDMI proof for scan-out claims | `documentation/`, catalog media |
| Input/controllers | `crates/magik-core/src/input_state.rs`, `apps/mister/src/input.rs` | `docs/device.md` | `$magik-rust-lsp` | Pre-push and CI | Attended controller test for Linux mappings | `apps/desktop/`, MiSTer platform rendering |
| Media/previews | `apps/mister/src/media_update.rs`, `crates/catalog/src/preview_worker.rs` | `docs/catalog.md` | `$magik-rust-lsp` | Pre-push and CI | Use private submodule tools for generated packs | `reference/`, raw device cache directories |
| Slint UI | `apps/mister/ui/launcher.slint` | `apps/mister/BUILD.md` | Slint MCP | Pre-push and CI select compiled UI/ARM checks | Visual validation for layout | `history/`, host tooling |
| Production particle scene | `crates/particles/`, `apps/mister/src/particle_renderer.rs`, `apps/framebuffer-scene-lab/` | `docs/startup-particles.md` | `$magik-rust-lsp` | Pre-push and CI select shared engine, focused lab, and ARM validation | macOS preview or attended Dev lab/launcher for visual and reload behavior | Slint UI; the separate 36-demo `apps/framebuffer-lab` showcase |
| Host and workflow tool | `agent-cli/src/` | `docs/device.md` | `$magik-rust-lsp` | Pre-push and native Linux CI | No device unless command execution is under test | `apps/desktop/`, `fpga/` |
| MagiK agent | `mister/tools/agent/src/main.rs` | `docs/magik-agent.md` | `$magik-rust-lsp` | Pre-push and native Linux CI | ARM/device for Linux-only operations | `documentation/`, launcher UI |
| Installer manager | `mister/tools/manager/src/main.rs` | `docs/installer.md` | `$magik-rust-lsp` | Pre-push and CI | ARM/device only after host fixtures pass | launcher rendering, catalog policy |
| Desktop UI | `apps/desktop/src/main.rs`, `apps/desktop/ui/main.slint` | `apps/desktop/AGENTS.md` | Rust analyzer and Slint MCP as applicable | Pre-push and macOS CI | Visual check for UI changes | MiSTer kernel/FPGA and device launcher internals |
| Documentation | `documentation/src/content/docs/` | `documentation/src/content/docs/contributing/` | None | Pre-commit, pre-push, and CI | None | Rust targets, device |
| Packaging/releases | `scripts/package-distribution.sh` | `docs/releases.md` | None | Pre-commit, pre-push, and CI | Operator gate: `scripts/agent release qualify` | UI runtime internals |
| Kernel scanout | `mister/platform/kernel/scanout-slots/` | `docs/kernel-scanout-plugin-assurance.md` | None | Pre-push and CI | Kernel build and attended device qualification | Catalog, desktop |
| FPGA latch | `mister/platform/fpga/menu-vblank-latch/` | `docs/fpga-latch-release.md` | None | Pre-push, typed local Apple FPGA signoff, and GitHub Actions RBF build | Quartus/device signoff | Catalog, documentation |

For dated evidence, search explicitly with `rg --no-ignore history/`. For
unknown work, run `scripts/agent plan` to preview the full affected assurance
plan.

The normal feature loop is edit with bounded analyzer feedback where applicable,
explicit `git add -- PATH...`, `git commit -m MESSAGE`, and `git push`. The
bootstrap-free Python pre-commit hook performs the fail-closed ten-second
policy, whitespace, syntax, and formatting gate. The pre-push hook enters
`agent-cli` for full affected assurance of a clean `HEAD`; CI remains
authoritative.
Runtime or platform changes then use `scripts/agent deliver`. Delivery uses the
exact clean local app commit. Platform delivery resolves the latest qualified
GitHub platform release and stages its Main, scanout kernel module, and latch
RBF together. The verified archive is cached by release tag and reused until
the latest tag changes. Reconciliation reads and verifies the installed
development manifest first, then selects `NoOp`, `Runtime`, or `Platform` from
all paths between its recorded app revision and `HEAD`. An installed artifact
mismatch forces a complete `Platform` repair. Delivery checks the latest GitHub
release tag before reconciliation but reuses its verified cached archive.
`NoOp` stops before builds or staging. `Runtime` updates the GUI and manifest as one
rollback-capable transaction without rebooting. `Platform` retains the complete
manifest-bound transaction for Main, kernel, FPGA, and contract changes.
Runtime and platform mutation each execute as one typed host transaction from
snapshot through health verification and commit or rollback. A nonblocking,
process-owned host lock prevents concurrent delivery and cannot leave stale
device state. There is no persistent delivery lease.

Verified component receipts and release archives live under
`build/agent-cache/` and survive delivery cleanup. Published platform bundles
and game databases are reused only when their immutable inputs and artifact
hashes still match. An unchanged manager is verified against the installed canonical
manifest, fetched through the typed host transport, and cached by SHA-256;
otherwise it is rebuilt under a strict receipt. Transient staging remains under
`build/agent-deploy/`.
# Deployment

“Build and deploy” maps to an ordinary Git commit followed by
`scripts/agent deliver`. The CLI owns platform qualification and device
transactions, and records
progress and evidence. Performance, diagnosis, and attended acceptance use the
typed `benchmark`, `diagnose`, and `release qualify` state machines.
