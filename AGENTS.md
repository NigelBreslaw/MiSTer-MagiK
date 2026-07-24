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
mister arming-status
```

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
- `mister/tools/host/` — host device tool; read its local `AGENTS.md`
- `mister/tools/agent/` — device agent; read its local `AGENTS.md`
- `apps/desktop/` — macOS companion; read `apps/desktop/AGENTS.md`
- `scripts/` — validation/deploy/benchmark tooling; read `scripts/AGENTS.md`
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
- Never use the Codex GitHub plugin for repository, issue, PR, or Actions work.
  Use `gh`.
- Agents use `scripts/agent deliver`, `benchmark`, or `diagnose` for device
  workflows. Attended human operations use typed commands in the Rust `mister`
  host binary; never raw SSH/SCP or generic remote-shell orchestration.
- Device workflows, Apple container, virtualization, and attended `mister`
  commands require first-attempt escalation using their direct repository
  command.
- On device timeout, refusal, route, or authentication failure, stop after the
  first typed attempt and report the device unavailable.
- Edit `MiSTer.ini` only through typed `mister` mutators or approved
  install/restore scripts.
- Apple Silicon ARM builds use Apple `container` by default. Do not switch to
  Docker/OrbStack unless explicitly comparing `MISTER_ARM_BUILD_BACKEND=cross`.
- Enable `.githooks/pre-commit` with
  `git config core.hooksPath .githooks`.
- Treat `private/magik-cloud` as its own repository: commit and push it first,
  then update only the parent gitlink.
- Never stage private screenshots, caches, archives, `.env`, `.wrangler/`,
  credentials, or files under ignored `private/test-fixtures/`.
- Treat `reference/` as read-only.

## Top-Level Commands

```bash
scripts/agent check
scripts/agent deliver
scripts/agent plan
scripts/agent verify
scripts/agent-linux-verify --paths mister/tools/host mister/tools/agent
scripts/agent benchmark
scripts/agent capture usb-video
scripts/agent diagnose
scripts/agent release qualify
git add -- path/to/file
git commit -m "Describe the completed change"
```

Use `check` at meaningful iteration boundaries. Batch related edits before
checking. Do not rerun `check` after every small patch, formatting correction,
or immediately obvious follow-up; rerun it when a coherent slice is ready, when
a failure could have multiple causes, or before handing work off. Argument-free
`check` and `verify` select all working-tree changes. `--paths` is reserved for
CI or diagnostics, and `verify --staged` is the Git pre-commit interface. Use
`scripts/agent db report`, not ad-hoc SQL, for workflow evidence analysis. Run
`agent-linux-verify` on Apple Silicon when Linux-only Rust or Linux-specific
Clippy behavior is in scope. It runs the normal verification harness inside the
repository Apple Linux image and caches its Linux Rust toolchain under
`/private/tmp`; invoke it with first-attempt escalation.
`deliver` only when the committed change has runtime or platform impact.
`release qualify` is an attended operator gate; run it only when explicitly
requested.
`capture usb-video` is a macOS-only native capture of the fixed `USB Video`
input. By default it writes a validated 1920x1080 JPEG under the OS temporary
directory unless `--output PATH` is supplied. With `--seconds N` it instead
writes a bounded 1920x1080 QuickTime movie for 1–60 seconds. Both modes print a
Markdown artifact link and refuse to overwrite explicit output paths.
Do not narrate successful operation counts or names: report only that validation
is running, passed, or failed with the actionable summary. Agents must not
construct Cargo, test, lint, host-validation, or Apple-container commands
directly; the harness selects, times, deduplicates, and records them.
“Build and deploy” means create the Git commit first, then
`scripts/agent deliver`; do not call
implementation scripts or supply deployment feature flags. `deliver` never
changes Git state or pushes. Development delivery builds the app runtime from
the exact clean local commit. Main, the scanout kernel module, and the latch RBF
must come together from the latest qualified GitHub platform release; reuse the
verified tag-addressed cache instead of downloading an unchanged release.
Git's index is the only commit-scope authority. Stage only intentional paths
with `git add -- PATH...`; never use broad staging when unrelated changes
exist. Invoke `git add`, `git commit`, and the one-time
`git config core.hooksPath .githooks` with first-attempt sandbox escalation
because they write `.git`. Persistent approvals must be limited to the narrow
`git add` and `git commit` prefixes, never unrestricted `git`. The trusted
pre-commit hook runs `scripts/agent verify --staged`; a failure leaves the
index staged for correction. Concurrent agents must use separate worktrees.

## Universal Hard Rules

- Never set `main=mister-magik-fb`; Slint cannot replace Main video
  initialization. Use `MiSTer_MagiK` or `MiSTer_MagiKDev`.
- Never launch cores with external `rbf_load`; use Main's command/FIFO handoff.
- Never SIGSTOP MiSTer for the launcher.
- Use Analytics live streaming for continuous framebuffer inspection and
  `mister --capture-buffer` for stills. Do not add raw
  `/dev/fb0` capture paths.
- `/dev/fb0` contents alone do not prove HDMI visibility.
- Production rendering is RGB565-only. Do not restore wider-color routes.
- Do not rebuild preview caches on the MiSTer hot path.
- Do not lower priority or pin CPU0 for initial catalog creation.
- The scanner must not walk screenshot/cache media, read `gamelist.xml`, or
  classify helper payloads as games.
- Use velocity scenarios, not row jumps, for arcade performance conclusions.

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
