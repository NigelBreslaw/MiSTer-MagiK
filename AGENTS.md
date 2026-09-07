# MiSTer MagiK

Preserve user changes. Stage exact paths with `git add -- PATH...`; commit with
`git commit -m MESSAGE`. Both require first-attempt sandbox escalation. Never
rewrite pushed history without an explicit request. Exclude secrets, credentials,
`.env`, `.wrangler/`, screenshots, caches, archives, ignored fixtures, and private
contents. Commit and push changed submodules before staging their gitlinks.

Use **MiSTer MagiK**, processes `MiSTer_MagiK`/`MiSTer_MagiKDev`, slug
`mister-magik`, package `mister-magik-fb`, crate `mister_magik_fb`. Never set
`main=mister-magik-fb` or introduce the retired spelling.

Portable logic belongs in `crates/`; hardware in `mister/`; device UI in
`apps/mister/`; Desktop in `apps/desktop/`; Python host orchestration in
`magik/host/magik/`; the native Rust service in `magik/agent/`. Keep `scripts/`
entrypoints thin. `reference/` is read-only; private submodules are independent.

## Development

Use `$magik-rust-lsp` for Rust/Cargo and Slint MCP for UI behavior. Run focused
checks through `scripts/cargo`. `scripts/magik-ci plan` previews validation;
`guidance PATH` reports ownership. Pre-commit checks the index; pre-push runs
bootstrap-free Python checks and affected tests. CI owns broad Rust/ARM/UI checks.
Use `gh` for GitHub. Dependency changes use `scripts/magik-ci dependencies sync
PATH/Cargo.toml`; stage only the owning manifest and adjacent lockfile.

## Device operations

Use `scripts/magik deploy`, `check`, and `watch`; real MagiK is the default,
Mini uses `--app mini-magik`. `check` is one smoke journey; measurements, profiles
and hardware checks are explicit. Preserve Dev/production separation and Main's
FIFO/load_core handoff. Dirty-worktree development delivery is supported.

Use `scripts/magik device` for control and catalog operations, and
`scripts/magik-platform` for platform/Main delivery. Discovery and Keychain
credentials are shared across worktrees. Reuse compatible service capabilities;
bootstrap or supply missing capabilities automatically within an authorized action.
Normal operations use native transport. SSH is limited to the fixed-purpose
bootstrap/repair adapter; never expose a generic shell or use raw SSH/SCP.

Device, Apple-container and virtualization commands require first-attempt
sandbox escalation. Use Apple `container`, never Docker. Reconcile ambiguous
mutations before proceeding; authentication failures require changed access.
Before recovery, read `docs/device.md#boot-loop-safety`. Keep recovery attended
and bounded; never add automatic reboot loops or persistent fault arming.

FPGA synthesis uses the GitHub platform workflow or typed Apple signoff.
Platform replacement remains transactional; FPGA activation is explicit.
Mutate MiSTer.ini through typed operations. Production rendering is RGB565;
use native Analytics and authoritative captures, never raw fb0 or hot-path previews.

Read applicable ancestor instructions and relevant source sections. Keep logs in
ignored artifacts and use history for provenance, not current operating guidance.
