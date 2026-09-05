# MiSTer MagiK

Preserve user changes. Stage exact paths with `git add -- PATH...`; commit with
`git commit -m MESSAGE`. Both require first-attempt sandbox escalation. Never
rewrite pushed history without an explicit request. Exclude secrets, credentials,
`.env`, `.wrangler/`, screenshots, caches, archives, ignored fixtures, and private
contents. Commit and push changed submodules before staging their gitlinks.

Use **MiSTer MagiK**, processes `MiSTer_MagiK`/`MiSTer_MagiKDev`, slug
`mister-magik`, package `mister-magik-fb`, crate `mister_magik_fb`. Never set
`main=mister-magik-fb` or introduce the retired spelling.

Portable domain/wire logic belongs in `crates/`; hardware in `mister/`; device
UI in `apps/mister/`; desktop UI in `apps/desktop/`; operational workflows in
`agent-cli/`; thin host entrypoints in `scripts/`. `reference/` is read-only;
private submodules are independent repositories.

## Development

Use `$magik-rust-lsp` for Rust/Cargo and Slint MCP for UI behavior. Run focused
checks, tests, and lints through `scripts/cargo`. `scripts/agent plan` previews
validation. Pre-commit checks the index; pre-push owns bootstrap-free Python
checks and affected Python tests, never Rust assurance. CI owns broad workspace,
host, ARM, and visual matrices; do not repeat these locally unless a typed
workflow requires them. Python `scripts/magik-ci` owns CI/release processing.

Use `gh` for GitHub. Dependency changes use
`scripts/agent dependencies sync PATH/Cargo.toml`, staging only the owning
manifest and adjacent lockfile.

## Operations

For ordinary application development, use `scripts/magik2 deploy`, `check`, and
`watch`; real MagiK is the default, Mini uses `--app mini-magik`. `check` runs
one smoke journey; request idle/motion measurements or profiling explicitly.
Use the 2.0 exception below. Do not route ordinary app development through 1.0.

For retained platform/release and legacy operations, use typed
`scripts/agent deliver`, `benchmark`, `diagnose`, and `db report`;
human device operations use attended `scripts/agent device`. Never use raw
SSH/SCP, generic remote shells, or ad-hoc SQL. Device, Apple-container, and
virtualization commands require first-attempt escalation. Retry read-only
requests once after transport failure; reconcile mutations. Authentication
failures require changed access.

Before reset/recovery work, read `docs/device.md#boot-loop-safety`: volatile
arming, bounded timeouts, interruption-safe cleanup, verified disarming, and
non-network recovery are mandatory. Never leave persistent/unattended reboot
loops or replay diagnosis's one-shot reboot; stop on reboot/SSH instability.

“Build and deploy” for retained 1.0 platform/release work means commit then deliver the exact clean app revision with
the latest qualified platform. Replace runtime and regenerated
`platform-v3.manifest` transactionally. Local Main delivery requires committed
Dev Main work. Release qualification requires an explicit attended request.

FPGA synthesis uses only the GitHub platform workflow or typed Apple Silicon
`fpga signoff`; local RBF activation requires an attended rollback transaction.
Use Apple `container`, never Docker. Launch through Main's FIFO/load_core with
the manifest-selected Dev RBF, never external loading, menu.rbf, or SIGSTOP.
Mutate `MiSTer.ini` only through typed/approved mutators.

Production rendering is RGB565; never rebuild previews on the hot path.
Separate latch rejection from physical repeats; authoritative animation requires
zero dropped frames. Use Analytics streaming or typed captures, never raw fb0.

## MiSTer MagiK Tooling 2.0 exception

The user-approved 2.0 plan authorizes the isolated `magik2/` project and its
thin `scripts/magik2` entrypoint. Within this scope, these rules take precedence
over conflicting legacy tooling requirements in this file and `scripts/AGENTS.md`:

- Python host orchestration belongs in `magik2/host/magik2`; the native Rust
  device service belongs in `magik2/agent`. Do not invoke or wrap the legacy
  agent CLI to build, bootstrap, deploy, test, profile, or observe 2.0.
- Use typed `scripts/magik2` operations. Their internal bootstrap/repair adapter
  may use SSH to install/start the native service when absent or unreachable,
  or repair it when native recovery is unavailable. This is the approved
  exception to the raw SSH/SCP prohibition; it does not authorize ad-hoc shell
  SSH/SCP commands or a generic remote-shell interface. Normal delivery,
  reachable-service upgrades, control, testing, profiling, and streams use
  the native connection.
- Within an authorized 2.0 operation, bootstrap and missing-capability updates
  happen automatically and then resume that operation. Build/version mismatch
  alone never requires an update. Keep a compatible installed service.
- Experiment delivery may use a dirty worktree and does not require a commit,
  qualified platform release, production manifest update, or rollback
  transaction. Scope installation to `/media/fat/mister-magik2` and temporary
  state to `/tmp/mister-magik2`; preserve the real app and installed platform.
- Use the new native metrics/frame streams and 2.0 viewer for observation.
  Existing Main handoff, RGB565 presentation, and no-raw-framebuffer rules
  still apply. Do not introduce automatic reboot recovery.
- Device and Apple-container operations through `scripts/magik2` still require
  first-attempt sandbox escalation. This transport approval does not bypass
  the sandbox. Reconcile ambiguous mutations instead of blindly replaying them.

## Context

Read applicable ancestor instructions and needed source/document sections.
`scripts/agent guidance PATH` reports ownership and references. Batch independent
reads; return bounded findings with failures, provenance, and truncation intact.
Keep full logs in ignored artifacts; read history only for provenance.
