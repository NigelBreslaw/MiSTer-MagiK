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
scripts/mister run "ls -l /media/fat/mister-magik*/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik*/rebuild-on-next-boot 2>/dev/null || true"
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

- `magik-gui/` — device frontend; read `magik-gui/AGENTS.md`
- `magik-gui/src/ui_runner/` — launcher runtime; read its local `AGENTS.md`
- `tools/mister/` — host device tool; read its local `AGENTS.md`
- `tools/magik-agent/` — device agent; read its local `AGENTS.md`
- `desktop/` — macOS companion; read `desktop/AGENTS.md`
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
- Use `scripts/mister` for all device communication; never raw SSH/SCP.
- Device, Apple container, virtualization, and MiSTer commands require
  first-attempt escalation using their direct repository command.
- On device timeout, refusal, route, or authentication failure, stop after the
  first wrapper attempt and report the device unavailable.
- Edit `MiSTer.ini` only through `scripts/mister` mutators or approved
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
scripts/validate working-tree --print-plan
scripts/validate working-tree
scripts/validate full-host
scripts/dev-rust test
scripts/dev-rust check
magik-gui/build-arm.sh --device
scripts/deploy-rust.sh
scripts/mister status
```

`scripts/validate affected` remains the staged-index pre-commit interface.
Use `scripts/validate paths PATH...` for explicit files or directories.

## Universal Hard Rules

- Never leave the MiSTer in a persistent boot loop.
- Never set `main=mister-magik-fb`; Slint cannot replace Main video
  initialization. Use `MiSTer_MagiK` or `MiSTer_MagiKDev`.
- Never launch cores with external `rbf_load`; use Main's command/FIFO handoff.
- Never SIGSTOP MiSTer for the launcher.
- Use Analytics live streaming for continuous framebuffer inspection and
  `scripts/mister agent framebuffer-capture` for stills. Do not add raw
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
- Current architecture: `docs/architecture.md`
- Catalog lifecycle: `docs/catalog.md`
- Device/recovery policy: `docs/device.md`
- Benchmark method: `docs/benchmarking.md`
- Main fork: `docs/main-mister-fork.md`
- ARM/build policy: `magik-gui/BUILD.md`

Agent-critical universal rules belong here. Subsystem rules belong in the
nearest `AGENTS.md`; current design belongs in `docs/`; dated evidence belongs
in `history/`.
