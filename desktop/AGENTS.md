# AGENTS.md - desktop app

Desktop-specific notes for agents working in this folder. The root
`AGENTS.md` still applies, especially device safety and `scripts/mister` rules.

## Current Shape

- This is the macOS desktop companion for MiSTer MagiK.
- The UI is Slint 1.17 and uses the live sibling Primer package at:
  `../../github-app/packages/primer-slint`.
- The main UI file is `ui/main.slint`.
- Rust owns data fetching, cache state, and Slint global updates in `src/main.rs`.
- SD Card browser state lives in `src/sd_card.rs`.
- The Analytics view captures framebuffer PNGs through the MagiK agent
  `framebuffer_capture` command. Keep desktop capture paths on that agent API;
  do not add raw `/dev/fb0` dump or host-side raw-to-PNG flows.
- Material file icons are loaded at runtime from a git submodule:
  `desktop/vendor/material-icon-theme`.

## Slint And MCP

- Prefer the live dev script while iterating:
  `SLINT_MCP_PORT=9315 desktop/scripts/dev-live.sh`
- `desktop/scripts/dev-live.sh` intentionally runs a release build because live
  reload behaves better there.
- Always visually verify UI changes with the Slint MCP server before calling
  them done. Query the live tree for structure and interactions; use screenshots
  when layout, color, or clipping needs actual visual inspection.
- The MCP endpoint is local, for example `http://127.0.0.1:9315/mcp`.
- For compiled UI validation use:
  `cargo check --manifest-path desktop/Cargo.toml --no-default-features --features compiled-ui`

## Primer Dependency

- Do not vendor Primer Slint into this repo. Import it from the sibling
  `github-app` checkout.
- If a desktop change exposes a reusable Primer component gap, fix it in
  `/Users/nigelb/slint/github-app/packages/primer-slint`.
- `TreeViewRow` is an explicit Slint struct. If adding a field, update every
  struct literal in the Primer gallery too, not just the desktop app.

## Material Icon Theme

- The Material icons are a git submodule, not copied files:
  `desktop/vendor/material-icon-theme`.
- After cloning this repo, initialize icons with:
  `git submodule update --init desktop/vendor/material-icon-theme`
- Icons should be loaded from disk at runtime with `slint::Image::load_from_path`
  or the local `file_icons` helper. Do not use large `@image-url(...)` tables for
  the icon theme; that bakes assets into generated code and bloats binaries.
- The desktop app can override the icon directory with
  `MISTER_MAGIK_DESKTOP_MATERIAL_ICON_DIR`.
- Keep file rows lightweight: store an icon key in state and resolve it to a
  cached image at the UI boundary.

## Validation

Run the focused checks before committing desktop UI work:

```bash
cargo test --manifest-path desktop/Cargo.toml
cargo check --manifest-path desktop/Cargo.toml --no-default-features --features compiled-ui
cargo check --manifest-path desktop/Cargo.toml --features slint/mcp,live-ui
```

If you change the sibling Primer package, also run the relevant check there,
usually:

```bash
pnpm --filter slint-gallery run typecheck
```
