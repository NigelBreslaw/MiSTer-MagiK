# AGENTS.md - mister-slint

Universal safety lives here. Before subsystem work, read the nearest
`AGENTS.md` and narrowest `docs/agents/task-map.md` row.

## Critical Boot-Loop Safety

Never leave the MiSTer in an unattended or persistent reboot loop. A fast reset
loop can make SSH unusable and require SD-card recovery.

Never use persistent `launcher.env` to arm destructive reset faults.
`direct-reset-no-sync` requires a volatile `/tmp` session token. Cleanup and
exit traps for destructive runners must remove:

- `/media/fat/mister-magik/launcher.env`
- `/media/fat/mister-magik-dev/launcher.env`
- `/tmp/mister-magik/fs-fault-launcher.env`
- `/tmp/mister-magik/fs-fault-session`
- `/tmp/mister-magik/fs-fault.json`
- `/media/fat/mister-magik/rebuild-on-next-boot`
- `/media/fat/mister-magik-dev/rebuild-on-next-boot`

Host wait and recovery loops require bounded local timeouts. Before a
reset-fault test, confirm non-network recovery and interruption-safe cleanup.
After `direct-reset-no-sync`, verify no arming file remains:

```bash
scripts/agent device arming-status
```

`scripts/agent diagnose` may clear those files and issue one raw Linux reboot
over SSH only when the installed platform is coherent and launcher health is
down. Never replay that reboot automatically. If rebooting repeats, stop normal
delivery, remove stale arming files, and use SD-card recovery plus
`/media/fat/mister-magik/bootlogs/main-reboot.log` when SSH is unstable.

## Names And Repository Routing

MiSTer MagiK is a Rust/Slint frontend for MiSTer FPGA. Main_MiSTer is normally
`../Main_MiSTer`; override with `MISTER_MAIN_DIR`.

- Product/UI: **MiSTer MagiK**
- Main processes: `MiSTer_MagiK`, `MiSTer_MagiKDev`
- Directory/script slug: `mister-magik`
- Slint binary/package: `mister-magik-fb`
- Rust crate/import: `mister_magik_fb`

Never introduce the retired `magic` spelling or mixed-case path variants.

- `apps/mister/` — device frontend; `apps/mister/src/ui_runner/` — launcher;
  read each local `AGENTS.md`
- `agent-cli/` — workflow/device tool; `mister/tools/agent/` — device agent
- `apps/desktop/` — macOS companion; `scripts/` — host tooling; read local
  guidance
- `private/magik-assets/`, `private/magik-cloud/` — independent repositories;
  read their local guidance
- `docs/` — current policy; `history/` — dated evidence; `reference/` —
  optional read-only research clones

Routine `rg` honors `.ignore`; use `rg --no-ignore` only when excluded trees are
in scope.

## Universal Workflow

- Preserve user changes. Never reset, checkout, clean, overwrite, or broadly
  stage unrelated work; concurrent agents use separate worktrees.
- Never amend pushed `main`. Rewriting pushed history requires an explicit
  request and identification of the remote history being replaced.
- Never use the Codex GitHub plugin for repository, issue, PR, or Actions work;
  use `gh`.
- Agents use `scripts/agent deliver`, `benchmark`, or `diagnose` for device
  workflows. Attended operations use typed `scripts/agent device` commands.
  Never use raw SSH/SCP or generic remote-shell orchestration.
- Device workflows, Apple container, virtualization, and attended `mister`
  commands require first-attempt sandbox escalation with the direct repository
  command.
- Retry a read-only typed request once after transient transport failure. Never
  replay mutation blindly; use reconciliation or compensation. Authentication
  failures require changed access. Report unavailable only after bounded
  recovery fails.
- Edit `MiSTer.ini` only through typed `mister` mutators or approved
  install/restore scripts.
- Apple Silicon ARM builds use Apple `container`; never substitute
  Docker/OrbStack.
- Commit and push private submodules before updating only the parent gitlink.
- Never stage private screenshots, caches, archives, `.env`, `.wrangler/`,
  credentials, or ignored `private/test-fixtures/` content.
- Existing `reference/` repos are read-only; new clones may be added there.

## FPGA Safety

RBF synthesis runs only through `Build MiSTer MagiK Platform` GitHub Actions or
the typed Apple Silicon `scripts/agent fpga signoff` workflow. Never invoke
Quartus, its installer, or FPGA build scripts directly.

A canonical local signoff set may be installed only to the Dev layout through
the attended rollback-capable `scripts/agent device fpga install-experimental`
transaction. Never copy a local RBF directly to the device. Local artifacts are
not release-qualified; production delivery requires the GitHub platform
workflow.

Experimental activation must use Main's `load_core` with the exact
manifest-selected Dev latch RBF. Never activate or verify it through root
`/media/fat/menu.rbf` or `load_core menu.rbf`; that stock-owned route may fall
back to stock Menu. `mister_magik_reload_main` restarts Dev Main but does not
prove a replacement RBF was configured. Remaining signoff and evidence rules
are in `docs/fpga-latch-release.md`.

## Canonical Commands And Assurance

```bash
scripts/agent plan
scripts/agent deliver
scripts/agent deliver local-main
scripts/agent benchmark
scripts/agent capture usb-video
scripts/agent diagnose
scripts/agent dependencies sync path/to/Cargo.toml
QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup
scripts/agent fpga signoff
scripts/agent device fpga install-experimental --rbf PATH --metadata PATH --signoff-report PATH --attended
scripts/agent release qualify
git add -- path/to/file
git commit -m "Describe the completed change"
```

For Rust/Cargo work, use `$magik-rust-lsp`; refresh diagnostics after coherent
edits. Never construct Cargo, test, lint, host-assurance, or Apple-container
commands. Dependency changes use the canonical sync command and commit only the
owning manifest plus adjacent lockfile.

`scripts/agent plan` previews assurance without execution. The pre-commit hook
is the index-only gate, and the pre-push hook owns full local assurance. The
native Linux CI owns Linux Rust/Clippy behavior. Never construct hook or CI
assurance commands. Report only running, passed, or failed with actionable
detail. Use `scripts/agent db report`, never ad-hoc SQL, for workflow evidence.

Enable hooks with `git config core.hooksPath .githooks`. Git's index is the only
commit-scope authority: use explicit `git add -- PATH...`. Run `git add`,
`git commit`, and hook configuration with first-attempt sandbox escalation;
persistent approvals cover only narrow `git add` and `git commit` prefixes.

## Delivery And Operator Boundaries

- “Build and deploy” means commit, then `scripts/agent deliver`. Never call
  implementation scripts or add deployment flags. Delivery uses the exact
  clean app commit and never changes Git or pushes.
- Ordinary delivery uses the latest qualified platform release. `deliver
  local-main` replaces only Dev Main and its manifest; it never targets
  production or synthesizes an RBF.
- Run delivery only for committed runtime or platform impact.
- `benchmark` is a closed read/profile/health interface on the coherent Dev
  runtime; never mutate artifacts or bypass it with lower-level transport.
- `release qualify` is an attended operator gate and requires an explicit user
  request.
- `capture usb-video` owns validated fixed-`USB Video` capture and never
  overwrites an explicit path.

## AI Inspection And Output

Follow `docs/agents/ai-efficiency.md`. Start narrow; for Rust use semantic LSP
before shell search. Initial reads stop at 150 lines and searches at 100 matches.

Routine tool output targets 1,200 tokens under the 3,000-token history ceiling.
Reduce broad output at source; never forward it unconditionally. If truncated
evidence is insufficient, make one focused expansion.

## Universal Hard Rules

- Never set `main=mister-magik-fb`; Slint cannot replace Main video
  initialization. Use `MiSTer_MagiK` or `MiSTer_MagiKDev`.
- Never replace `mister-magik-fb` without its regenerated
  `platform-v3.manifest` in the coherent rollback-capable delivery transaction.
  Main suspend/resume acknowledgement is not launcher health.
- Never launch cores with external `rbf_load`; use Main's command/FIFO handoff.
- Never SIGSTOP MiSTer for the launcher.
- Use Analytics live streaming for continuous framebuffer inspection and
  `mister --capture-buffer` for stills. Never add raw `/dev/fb0` capture paths;
  framebuffer contents alone do not prove HDMI visibility.
- Never infer frame cadence from latch drops: they are rejected/superseded
  protocol posts, while dropped frames are physical refresh intervals reusing
  the prior frame. Authoritative animation requires zero dropped frames;
  measure both independently.
- Production rendering is RGB565-only. Never restore wider-color routes.
- Never rebuild preview caches on the MiSTer hot path.

## Sources Of Truth

- AI routing and efficiency: `docs/agents/task-map.md`,
  `docs/agents/ai-efficiency.md`
- File authority/regeneration: `docs/agents/file-authority.md`
- Architecture: `docs/architecture.md`
- Catalog: `docs/catalog.md`
- Device/recovery: `docs/device.md`
- Benchmarking: `docs/benchmarking.md`
- Main fork: `docs/main-mister-fork.md`
- ARM/build: `apps/mister/BUILD.md`
- FPGA release: `docs/fpga-latch-release.md`

Agent-critical universal rules belong here. Subsystem rules belong in the
nearest `AGENTS.md`; current design belongs in `docs/`; dated evidence belongs
in `history/`.
