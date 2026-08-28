# AGENTS.md - mister-slint

Use these universal rules plus the nearest scoped `AGENTS.md`. Other links are
references: open only the needed section and never follow recursively unless
blocked.

## Safety

- Preserve user changes. Never reset, checkout, clean, overwrite, or broadly
  stage unrelated work. Never rewrite pushed history without an explicit
  request identifying the history to replace.
- Never leave the MiSTer in an unattended or persistent reboot loop. Destructive
  reset tests require volatile `/tmp` arming, bounded host timeouts,
  interruption-safe cleanup, and confirmed non-network recovery.
- Cleanup for destructive runners must remove
  `/media/fat/mister-magik{,-dev}/{launcher.env,rebuild-on-next-boot}` and
  `/tmp/mister-magik/{fs-fault-launcher.env,fs-fault-session,fs-fault.json}`.
  After `direct-reset-no-sync`, run
  `scripts/agent device arming-status` and verify nothing remains armed.
- `scripts/agent diagnose` may clear stale arming state and issue one raw Linux
  reboot only when the installed platform is coherent and launcher health is
  down. Never replay that reboot automatically. Stop and use SD-card recovery
  plus `bootlogs/main-reboot.log` if rebooting repeats or SSH is unstable.
- Device, Apple-container, virtualization, and attended `scripts/agent device`
  commands require first-attempt sandbox escalation. Retry a read-only typed
  request once after transient transport failure; reconcile mutations instead
  of replaying them. Authentication failures require changed access.
- Never stage screenshots, caches, archives, secrets, `.env`, `.wrangler/`,
  credentials, ignored fixtures, or private-repository contents. Commit and
  push a private submodule before staging its parent gitlink.

## Names And Ownership

Use **MiSTer MagiK**, processes `MiSTer_MagiK` and `MiSTer_MagiKDev`, slug
`mister-magik`, binary/package `mister-magik-fb`, and Rust crate
`mister_magik_fb`. Never introduce the retired `magic` spelling.

Portable domain logic belongs in `crates/`; MiSTer hardware integration in
`mister/`; device UI in `apps/mister/`; macOS UI in `apps/desktop/`; typed host
workflow in `agent-cli/`; thin host entrypoints in `scripts/`. Existing
`reference/` repositories are read-only. Private submodules are independent
repositories.

## Workflow

- Use `$magik-rust-lsp` for Rust/Cargo navigation and diagnostics. Use the Slint
  MCP for Slint behavior. Do not construct Cargo, test, lint, hook, host
  assurance, ARM, or Apple-container commands; pre-commit, pre-push, native Linux
  CI, and other CI groups own full assurance.
- `scripts/agent plan` previews affected assurance. Agents use the typed
  `scripts/agent deliver`, `benchmark`, and `diagnose` workflows. Human device
  operations use attended `scripts/agent device` commands. Never use raw
  SSH/SCP or generic remote-shell orchestration. Use `scripts/agent db report`,
  never ad-hoc SQL.
- Dependency changes use `scripts/agent dependencies sync PATH/Cargo.toml` and
  include only the owning manifest plus adjacent lockfile.
- Stage exact paths with `git add -- PATH...` and commit with
  `git commit -m MESSAGE`; both require first-attempt sandbox escalation. The
  pre-commit hook is the index-only gate and the pre-push hook owns full
  clean-`HEAD` assurance. Use `gh`, never the Codex GitHub plugin.
- “Build and deploy” means commit, then `scripts/agent deliver`. Delivery uses
  the exact clean app commit and the latest qualified platform. Use
  `scripts/agent deliver local-main` only for committed Dev Main work.
  `scripts/agent release qualify` is an attended operator gate requiring an
  explicit request.
- FPGA synthesis runs only through GitHub `Build MiSTer MagiK Platform` or the
  typed Apple Silicon `scripts/agent fpga signoff` workflow. Never invoke
  Quartus or FPGA build scripts directly. Local RBFs may be installed only by
  the attended rollback-capable Dev transaction, never copied to the device.

## Hard Invariants

- Never set `main=mister-magik-fb`; use `MiSTer_MagiK` or `MiSTer_MagiKDev`.
- Never replace `mister-magik-fb` without its regenerated
  `platform-v3.manifest` in one rollback-capable delivery transaction.
- Launch cores through Main's command/FIFO handoff, never external `rbf_load`.
  Never SIGSTOP MiSTer for the launcher.
- Experimental FPGA activation uses Main's `load_core` with the exact
  manifest-selected Dev latch RBF, never `/media/fat/menu.rbf`.
- Use Analytics streaming for continuous framebuffer inspection and typed
  `mister --capture-buffer` for stills. Never add raw `/dev/fb0` capture paths.
- Measure latch rejection separately from physical dropped frames. Authoritative
  animation requires zero dropped frames. Production rendering is RGB565-only,
  and preview caches must never rebuild on the MiSTer hot path.
- Edit `MiSTer.ini` only through typed mutators or approved install/restore
  scripts. Apple Silicon ARM work uses Apple `container`, never Docker.

## Context Discipline

Start with source and nearest scoped rules. `scripts/agent guidance PATH`
reports authority, regeneration, one canonical reference, and extra assurance.
Initial reads stop at 150 lines and searches at 100 matches. Routine tool output
stays under 1,200 tokens and stored output under 3,000 tokens. Reduce broad
output at its source; never forward unconditional broad `r.output`. Make one
focused expansion only when needed. Read `history/` only for provenance.
