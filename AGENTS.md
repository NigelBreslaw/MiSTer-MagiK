# AGENTS.md - mister-slint

Read this first. This root file contains universal safety and repository rules.
Subsystem-specific entrypoints and checks live in the nearest `AGENTS.md`.

## Critical Boot-Loop Safety

Highest priority: never leave the MiSTer in an unattended or persistent reboot
loop. A fast reset loop can make SSH unusable and may require pulling the SD
card to recover.

Never use persistent `launcher.env` to arm destructive reset faults.
`direct-reset-no-sync` must require a volatile `/tmp` session token. Cleanup
and exit traps for destructive runners must remove:

- `/media/fat/mister-magik/launcher.env`
- `/media/fat/mister-magik-dev/launcher.env`
- `/tmp/mister-magik/fs-fault-launcher.env`
- `/tmp/mister-magik/fs-fault-session`
- `/tmp/mister-magik/fs-fault.json`
- `/media/fat/mister-magik/rebuild-on-next-boot`
- `/media/fat/mister-magik-dev/rebuild-on-next-boot`

Host wait/recovery loops must use bounded local timeouts. Before any reset-fault
test, confirm a non-network recovery path and interruption-safe cleanup. After
direct-reset-no-sync experiments, verify no live arming file remains:

```bash
scripts/agent device arming-status
```

A single bounded recovery reboot is not a reboot loop. `scripts/agent diagnose`
may clear the listed arming files and issue one raw Linux reboot over SSH when
the installed platform is coherent and the launcher or its heartbeat is down.
That reboot request must never be automatically replayed.

If the MiSTer repeatedly reboots, stop normal deploy attempts. Remove stale
arming files first; if SSH is unstable, power down, mount the SD card on the
Mac, remove them directly, and inspect
`/media/fat/mister-magik/bootlogs/main-reboot.log`.

## Product And Canonical Names

MiSTer MagiK is a Rust/Slint frontend for MiSTer FPGA. The maintained
Main_MiSTer fork is normally at `../Main_MiSTer`; override with
`MISTER_MAIN_DIR`.

- Product/UI text: **MiSTer MagiK**
- Main binaries/processes: `MiSTer_MagiK`, `MiSTer_MagiKDev`
- Directory/script slug: `mister-magik`
- Slint binary/package: `mister-magik-fb`
- Rust crate/import: `mister_magik_fb`

Do not introduce the retired `magic` spelling or mixed-case path variants.

## Repository Routing

- `apps/mister/` — device frontend; read `apps/mister/AGENTS.md`
- `apps/mister/src/ui_runner/` — launcher runtime; read its local `AGENTS.md`
- `agent-cli/` — unified workflow and typed device tool
- `mister/tools/agent/` — device agent; read its local `AGENTS.md`
- `apps/desktop/` — macOS companion; read `apps/desktop/AGENTS.md`
- `scripts/` — validation/deploy/benchmark tooling; read `scripts/AGENTS.md`
- `private/magik-assets/` — licensed build assets; read its local `AGENTS.md`
- `private/magik-cloud/` — private submodule; read its local `AGENTS.md`
- `docs/` — current engineering policy
- `history/` — dated evidence, not current policy unless linked
- `reference/` — optional read-only research clones

Routine `rg` searches skip history, references, vendored dependencies, and
generated/build output through `.ignore`. Use `rg --no-ignore` explicitly when
those trees are part of the task.

## Universal Workflow Rules

- Preserve user changes. Never reset, checkout, clean, or overwrite unrelated
  work.
- Never amend commits on `main` after they have been pushed. Once history is
  published, add a new commit instead. Rewriting pushed branch history, including
  force-push or amend-and-force-push, requires an explicit user request and a
  clear statement of the remote state being replaced.
- Never use the Codex GitHub plugin for repository, issue, PR, or Actions work.
  Use `gh`.
- Agents use `scripts/agent deliver`, `benchmark`, or `diagnose` for device
  workflows. Diagnosis owns bounded read-only retries and one unattended
  one-shot recovery reboot. Attended operator operations use typed
  `scripts/agent device` commands; never raw SSH/SCP or generic remote-shell
  orchestration.
- Device workflows, Apple container, virtualization, and attended `mister`
  commands require first-attempt escalation using their direct repository
  command.
- Retry an explicitly read-only typed request once after a transient timeout,
  refusal, or route failure. Never blindly replay mutation: use the owning
  workflow's reconciliation or compensation path. Authentication and access
  failures require changed credentials or permissions before retrying. Report
  the device unavailable only after the bounded recovery path fails.
- Edit `MiSTer.ini` only through typed `mister` mutators or approved
  install/restore scripts.
- Apple Silicon ARM builds use Apple `container` by default. Do not switch to
  Docker/OrbStack.
- RBF synthesis runs either in the `Build MiSTer MagiK Platform` GitHub Actions
  workflow or through the typed Apple Silicon `scripts/agent fpga signoff`
  workflow. Never invoke Quartus, its installer, or the FPGA build script
  directly. Local RBFs are ignored signoff-cache artifacts and are never
  deployable or release-qualified by the local workflow.
- Enable `.githooks/pre-commit` with
  `git config core.hooksPath .githooks`.
- Treat `private/magik-assets` and `private/magik-cloud` as their own
  repositories: commit and push them first, then update only the parent
  gitlink.
- Never stage private screenshots, caches, archives, `.env`, `.wrangler/`,
  credentials, or files under ignored `private/test-fixtures/`.
- Treat repos in `reference/` as read-only. However you can clone new repos into the folder.

## Top-Level Commands

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
scripts/agent release qualify
git add -- path/to/file
git commit -m "Describe the completed change"
```

For Rust or Cargo work, use the repo-scoped `$magik-rust-lsp` skill for
semantic navigation and package-scoped Clippy diagnostics. Refresh diagnostics
after a coherent edit batch, not after every small patch. Do not construct
Cargo, test, lint, host-assurance, or Apple-container commands directly.
For dependency changes, edit the owning `Cargo.toml`, run
`scripts/agent dependencies sync path/to/Cargo.toml` (optionally with
`--package NAME` for a focused update), review the manifest and adjacent
`Cargo.lock`, then stage and commit those exact files together. The sync command
uses standard `cargo update` and verifies the resulting all-features graph with
`cargo metadata --locked`.
`scripts/agent plan` previews the full assurance selected for working-tree or
explicit paths without executing it. The pre-commit hook is the index-only fast
gate, the pre-push hook is the full affected local assurance interface, and
native Linux CI owns Linux-specific Rust and Clippy behavior. Use
`scripts/agent db report`, not ad-hoc SQL, for workflow evidence analysis.
`deliver` only when the committed change has runtime or platform impact.
`benchmark` must use the coherently installed Dev runtime and may never build,
upload, swap, or restore an executable or manifest. The benchmark client is a
closed read/profile/health interface; do not bypass it with host or agent
transport calls.
`release qualify` is an attended operator gate; run it only when explicitly
requested.
`capture usb-video` is a macOS-only native capture of the fixed `USB Video`
input. By default it writes a validated 1920x1080 JPEG under the OS temporary
directory unless `--output PATH` is supplied. With `--seconds N` it instead
writes a bounded 1920x1080 QuickTime movie for 1–60 seconds. Both modes print a
Markdown artifact link and refuse to overwrite explicit output paths.
`fpga signoff` resolves the current local `main` ref, builds stock Menu, the
exact pinned pre-observer revision, and final Main inputs in generated isolated
checkouts, then runs the unchanged Quartus delta checker. Completed synthesis
is cached before validation, so an expected delta failure can be rerun without
rebuilding. Set `MISTER_FPGA_LOCAL_ROOT` to a stable absolute directory when
multiple worktrees must share the Quartus install and synthesis cache.
Do not narrate successful operation counts or names: report only that validation
is running, passed, or failed with the actionable summary. Agents must not
construct hook or CI assurance commands directly; those boundaries select and
time their operations, while pre-push and CI also deduplicate and record them.
“Build and deploy” means create the Git commit first, then
`scripts/agent deliver`; do not call
implementation scripts or supply deployment feature flags. `deliver` never
changes Git state or pushes. Development delivery builds the app runtime from
the exact clean local commit. Ordinary `deliver` takes Main, the scanout kernel
module, and the latch RBF together from the latest qualified GitHub platform
release. The permanent Dev-only `deliver local-main` exception builds the exact
clean sibling `Main_MiSTer` commit and replaces only Dev Main plus its
regenerated manifest; it must preserve the verified installed app, manager,
module, and RBF identities. It never targets production or synthesizes an RBF.
Git's index is the only commit-scope authority. Stage only intentional paths
with `git add -- PATH...`; never use broad staging when unrelated changes
exist. Invoke `git add`, `git commit`, and the one-time
`git config core.hooksPath .githooks` with first-attempt sandbox escalation
because they write `.git`. Persistent approvals must be limited to the narrow
`git add` and `git commit` prefixes, never unrestricted `git`. The trusted
pre-commit hook runs the bootstrap-free Python fast gate under a strict
ten-second deadline; a failure leaves the index staged for correction. The
pre-push hook runs full affected verification before branch updates reach the
remote. Concurrent agents must use separate worktrees.

## Universal Hard Rules

- Never set `main=mister-magik-fb`; Slint cannot replace Main video
  initialization. Use `MiSTer_MagiK` or `MiSTer_MagiKDev`.
- Never replace `mister-magik-fb` without its regenerated
  `platform-v3.manifest` in the coherent rollback-capable delivery transaction.
  A Main suspend/resume acknowledgement is not launcher health.
- Never launch cores with external `rbf_load`; use Main's command/FIFO handoff.
- Never SIGSTOP MiSTer for the launcher.
- Use Analytics live streaming for continuous framebuffer inspection and
  `mister --capture-buffer` for stills. Do not add raw
  `/dev/fb0` capture paths.
- `/dev/fb0` contents alone do not prove HDMI visibility.
- Never infer frame cadence from FPGA latch drop counters. A latch drop is a
  rejected or superseded protocol post; a dropped frame is a physical refresh
  interval that displayed the previous frame because no new frame was
  confirmed. An authoritative animation window requires exactly zero dropped
  frames. Qualification and status reports must measure dropped frames and
  latch drops independently.
- Production rendering is RGB565-only. Do not restore wider-color routes.
- Do not rebuild preview caches on the MiSTer hot path.

## Sources Of Truth

- AI task routing: `docs/agents/task-map.md`
- File authority/regeneration: `docs/agents/file-authority.md`
- Current architecture: `docs/architecture.md`
- Catalog lifecycle: `docs/catalog.md`
- Device/recovery policy: `docs/device.md`
- Benchmark method: `docs/benchmarking.md`
- Main fork: `docs/main-mister-fork.md`
- ARM/build policy: `apps/mister/BUILD.md`

Agent-critical universal rules belong here. Subsystem rules belong in the
nearest `AGENTS.md`; current design belongs in `docs/`; dated evidence belongs
in `history/`.
