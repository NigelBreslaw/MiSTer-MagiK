# AGENTS.md - mister-slint

Read this first. This file is the short agent bootstrap: current shape, safe
commands, hard rules, and links to deeper docs. Keep it concise; move dated
experiments and long rationale into `docs/` or `history/`.

## Critical Boot-Loop Safety

Highest priority: never leave the MiSTer in an unattended or persistent reboot
loop. A fast reset loop can make SSH unusable and may require pulling the SD
card to recover. If there is any chance your change or test can reboot the
device repeatedly, design and verify a one-boot or volatile-only arming path
before running it.

The known dangerous failure mode is a persistent
`/media/fat/mister-magik/launcher.env` that arms
`MISTER_FS_FAULT_ACTION=direct-reset-no-sync`. If MagiK reads that env on every
boot, it can repeatedly send `mister_magik_direct_reset_no_sync` through
`/dev/MiSTer_cmd`. Prevent this exact trap:

- Never use persistent `launcher.env` to arm destructive reset faults. Use a
  `/tmp` env file or a direct one-shot command.
- `direct-reset-no-sync` fault injection must require a volatile `/tmp` session
  token, such as `/tmp/mister-magik/fs-fault-session`, so a stale env file is
  inert after reboot.
- Cleanup and exit traps for destructive runners must remove both persistent
  and volatile arming files:
  `/media/fat/mister-magik/launcher.env`,
  `/tmp/mister-magik/fs-fault-launcher.env`,
  `/tmp/mister-magik/fs-fault-session`,
  `/tmp/mister-magik/fs-fault.json`, and
  `/media/fat/mister-magik/rebuild-on-next-boot`.
- Host-side wait/recovery loops must use bounded local timeouts. Do not let a
  half-open SSH connection hang the runner while the MiSTer is flapping.
- Before running any reset-fault test, confirm the runner has a non-network
  recovery story and a cleanup path that works if interrupted. If the SD card is
  mounted on the Mac, remove stale arming files directly before booting again.
- After any direct-reset-no-sync experiment, verify there is no live arming file
  with `scripts/mister run "ls -l /media/fat/mister-magik/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik/rebuild-on-next-boot 2>/dev/null || true"`.
- If the MiSTer starts rebooting repeatedly, stop trying normal deploys. First
  break the loop by removing stale arming files; if SSH is unstable, power down,
  mount the SD card on the Mac, remove the files above, and inspect
  `/media/fat/mister-magik/bootlogs/main-reboot.log`.

## Current State

MiSTer MagiK is a Rust/Slint frontend for MiSTer FPGA. The app renders a 960x540
software UI into `/dev/fb0`; the FPGA scales it to 1080p HDMI. The Rust app owns
the launcher framebuffer mode and scan-out route while it is active.

Production boot uses stock `/media/fat/MiSTer` from `/etc/inittab`, then
`[MiSTer] main=MiSTer_MagiK` re-execs the external Main_MiSTer fork. That fork
initializes HDMI/video and starts:

```text
/media/fat/mister-magik/mister-magik-fb ui launcher 0
```

The maintained Main_MiSTer fork is a sibling checkout at `../Main_MiSTer` by
default. Override with `MISTER_MAIN_DIR`.

## Canonical Names

- Product/UI text: **MiSTer MagiK**.
- Main_MiSTer fork binary/process/device path: `MiSTer_MagiK`.
- Slug for directories and scripts: `mister-magik`.
- Slint framebuffer binary/package: `mister-magik-fb`.
- Rust crate/import spelling: `mister_magik_fb`.
- Do not introduce the old `magic` spelling or mixed-case path variants.

## Repo Map

- `magik-gui/` - Rust/Slint frontend and device binary.
- `magik-gui/ui/` - Slint UI, fonts, art, and benchmark scenes.
- `magik-gui/BUILD.md` - ARM build profiles, FFmpeg, size, CI, toolchain notes.
- `tools/mister/` - Rust host-side MiSTer SSH/status/cache tooling.
- `scripts/` - approved build, deploy, profiling, and device wrappers.
- `private/magik-cloud/` - private submodule for screenshot source data,
  raw565 cache generation, `.mmlz4b` pack building, and Cloudflare R2 publish
  tooling. Source is private; the public repo tracks only the gitlink.
- `docs/architecture.md` - current architecture and handoff model.
- `docs/catalog.md` - catalog lifecycle, stamp validation, build/publish model,
  and benchmark gates.
- `docs/device.md` - MiSTer facts, INI policy, recovery, framebuffer/audio notes.
- `docs/benchmarking.md` - benchmark scenario policy and profiling commands.
- `docs/main-mister-fork.md` - external Main_MiSTer fork source of truth.
- `history/` - dated experiment logs and evidence; not current policy by default.
- `reference/` - gitignored research clones; read-only, optional context.

## Workflow Rules

- Use `scripts/mister` for all device comms. Do not use raw ssh or raw scp.
- The wrapper defaults to `MISTER_IP=192.168.1.117` and `MISTER_PASS=1`; avoid
  inline environment assignments unless intentionally targeting another MiSTer.
- Prefer direct commands such as `scripts/mister ...`, `scripts/deploy-rust.sh`,
  and `scripts/bench-toolchain.sh ...`. Avoid `/bin/zsh -lc` wrappers for normal
  device work because sandbox approvals key off the outer command.
- `scripts/deploy-rust.sh` deploys the MagiK binary through the agent by
  default. It is runtime-only; build/publish catalog metadata and screenshot
  packs with the catalog/media tools, not deploy flags. Use
  `MISTER_DEPLOY_TRANSPORT=ssh` only as an explicit fallback test or recovery
  path.
- For catalog database inspection, use the direct query helpers first:
  `scripts/mister db "SELECT ..."` or `scripts/mister library-db "SELECT ..."`.
  Do not assume the MiSTer has `sqlite3`, and do not pull
  `/media/fat/mister-magik/library.sqlite3` just to inspect rows unless the
  direct query path is unavailable. If `scripts/mister db` reports an unknown
  `library-sql` command, treat it as a host-tool/deployed-binary mismatch to
  fix, not as proof that direct DB querying is unsupported.
- Local ARM builds on Apple Silicon use Apple's `container` runtime by default.
  Do not route local builds through Docker/OrbStack unless
  `MISTER_ARM_BUILD_BACKEND=cross` is explicitly requested for a comparison.
- The repo tracks `.githooks/pre-commit`; enable it per clone with
  `git config core.hooksPath .githooks` so local commits run the fast host CI
  gates. See `magik-gui/BUILD.md`.
- Edit `MiSTer.ini` only through `scripts/mister` mutators or the provided
  install/restore scripts. Do not use ad hoc sed/awk/manual rewrites.
- Use `scripts/magik-cloud path` or `scripts/magik-cloud run -- ...` for
  magik-cloud commands. It resolves `MAGIK_CLOUD_DIR`, then the private
  `private/magik-cloud` submodule, then legacy `../magik-cloud`.
- Treat `private/magik-cloud` as its own private git repo. If you edit files
  there, commit and push inside the submodule first, then stage the parent
  submodule gitlink. Do not leave the parent pointing at an unpublished private
  commit.
- Never stage private source screenshots, generated caches, snapshot archives,
  `.env`, `.wrangler/`, or Cloudflare credentials. The parent repo should see
  only the submodule gitlink, not private file contents.
- Local real-device test fixtures may live under ignored `private/test-fixtures/`;
  use them for optional validation only and never stage their contents.
- Treat `reference/` as read-only. Do not commit changes there.
- Preserve user changes. Do not reset, checkout, or clean unrelated work.

## Command Shortlist

Host validation:

```bash
scripts/dev-rust test
scripts/dev-rust check
scripts/dev-rust host-tools
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```

Build and deploy:

```bash
magik-gui/build-arm.sh --device
scripts/deploy-rust.sh
scripts/deploy-main-mister-experiment.sh
scripts/deploy-main-mister-experiment.sh --clean-main  # only if stale Main objects are suspected
```

Run and inspect the deployed UI:

```bash
scripts/run-rust.sh launcher 0
scripts/run-rust.sh arcade 0
scripts/mister status
scripts/mister db
scripts/mister db "SELECT count(*) FROM games"
scripts/device-catalog-acceptance.sh
```

Boot and recovery:

```bash
scripts/install-slint-boot.sh
scripts/restore-stock-boot.sh
scripts/mister reboot-wait
scripts/mister reboot-wait --direct-reset  # fast dev reboot after writes are synced/stopped
scripts/mister recover
```

Keep plain `scripts/mister reboot-wait` for settings, INI/video-mode changes,
release gates, and unknown write state. Use `--direct-reset` for fast dev-loop
reboots only after file writes are complete and synced. Do not use
`--direct-reset-no-sync` outside explicit attended experiments.

Benchmark entrypoints:

```bash
scripts/profile-arcade-scroll.sh LABEL
scripts/profile-preview-scroll.sh LABEL
scripts/profile-first-preview.sh LABEL --skip-build
scripts/gate-preview-60fps.sh LABEL --skip-build --visual-captures 0
scripts/bench-toolchain.sh LABEL --replace-label
scripts/profile-first-scan.sh LABEL --deploy-device --replace-label
scripts/profile-library-io.sh LABEL --replace-label
```

See `docs/benchmarking.md` before drawing performance conclusions.
Effect-scene and mega-transition work is experimental only; see
`docs/experiments/effects.md`.

## Hard Rules

- Under no circumstances may an experiment, deploy, launcher env, reboot helper,
  or fault-injection runner leave the MiSTer in a persistent boot loop. Follow
  `Critical Boot-Loop Safety` before any direct-reset-no-sync work.
- Do not set `main=mister-magik-fb`. Slint is not Main; it cannot initialize
  HDMI before Main's `video_init()`. Use `main=MiSTer_MagiK`.
- Do not launch cores with external `rbf_load` from Slint. Use Main's command
  path / fifo handoff (`load_core` or `mister_magik_launch`) so HDMI survives.
- Do not SIGSTOP MiSTer for the launcher. A stopped Main can keep evdev grabs
  and the menu OSD over the framebuffer.
- Use the desktop Analytics live stream for continuous framebuffer inspection;
  it consumes producer-side `framebuffer_stream_v1` frames from
  `mister-magik-fb` through the MagiK agent. Capture still PNGs through the
  MagiK agent only with
  `scripts/mister agent framebuffer-capture OUT.png --json OUT.json`; do not
  add raw `/dev/fb0` dump or host-side raw-to-PNG paths. Agent captures still
  are not proof of HDMI output because `/dev/fb0` can contain Slint while the
  FPGA is scanning another buffer; confirm route/status or HDMI when visibility
  matters.
- Do not use row-by-row selected-index jumps for arcade performance conclusions.
  Use velocity scenarios from `docs/benchmarking.md`.
- Use RGB565 for launcher/arcade performance conclusions. The app render
  contract is RGB565-only; do not reintroduce wider-color env overrides, smoke
  commands, or framebuffer color-route A/B paths.
- Do not rebuild or write preview caches on the MiSTer hot path. Build source
  screenshots, raw565 caches, and snapshot packs from the private
  `private/magik-cloud` submodule. Use `scripts/magik-cloud path` to resolve
  `MAGIK_CLOUD_DIR`, the submodule, or the legacy `../magik-cloud` checkout.
- Do not lower priority or pin CPU0 for the initial catalog scan/database
  creation path. The first builder must run foreground with full CPU priority to
  meet first-scan gates; scan-screen frame drops are acceptable while no usable
  catalog exists.
- The library scanner must not walk screenshot/cache media directories, read
  `gamelist.xml`, or classify helper payloads as games.

## Device Facts

- Device: `192.168.1.117`, SSH `root` / `1`, static IP.
- CPU/OS: ARM Cortex-A9 `armv7l`, glibc 2.31, 1 GiB RAM.
- Framebuffer: `/dev/fb0`, driver `MiSTer_fb`, no DRM/KMS.
- 32-bit framebuffer byte order is B,G,R,X.
- Linux-side HDMI audio uses `/dev/MrAudio`, not ALSA.
- `/media/fat` is exFAT/FUSE; many small writes are slow.
- MiSTer busybox has no `pkill`; use `pidof`/`kill` through scripts.

See `docs/device.md` for INI policy, recovery, and hardware details.

## Architecture Notes

- Rust owns framebuffer mode/routing during the launcher. Main initializes HDMI,
  runs Rust `early-black`, starts Slint on `tty2`, then stays dormant.
- Slint renders into cached RAM and copies dirty regions to the small
  write-combined `/dev/fb0`. Do not reintroduce direct Slint rendering into live
  framebuffer memory for production UI.
- Game launch hands back to Main. On launch failure, Main/Slint recovery should
  restore launcher display and input rather than leaving HDMI wedged.
- Current open areas: live display geometry generalization, return-to-launcher
  after game reset, and controller mapping/hot-plug polish.

See `docs/architecture.md` for the full current model and history links.

## When Adding Knowledge

- Agent-critical rules go in this file only if they affect most future sessions.
- Current architecture goes in `docs/architecture.md`.
- Device procedures and recovery go in `docs/device.md`.
- Benchmark method and scenario policy go in `docs/benchmarking.md`.
- Dated experiments, failed approaches, and measurement logs go in `history/`.
