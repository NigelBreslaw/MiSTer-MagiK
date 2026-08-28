# AGENTS.md - desktop companion

## Ownership

Slint UI starts at `ui/main.slint`; Rust state and global updates start at
`src/main.rs`; SD-card browsing lives in `src/sd_card.rs`. Primer Slint and
Material icons are git submodules under `vendor/`.

## Rules

- Analytics live mode proxies producer-side RGB565 frames through the MagiK
  agent. One-shot capture uses the typed capture API. Never add raw `/dev/fb0`
  or host-side raw-to-PNG paths.
- Reusable Primer fixes belong in
  `vendor/github-app/packages/primer-slint`. When changing `TreeViewRow`, update
  every struct literal, including the gallery.
- Load Material icons from disk through `slint::Image::load_from_path` or the
  local helper. Do not bake large icon tables into generated code.
- Commit and push a changed vendor submodule before updating its gitlink.

## UI Feedback

Use `apps/desktop/scripts/dev-live-mcp.sh` for a live MCP session and
`apps/desktop/scripts/mcp-smoke.sh` for one endpoint check. Both require
first-attempt escalation. Do not improvise environment-prefixed launch commands
or retry on alternate ports. Use `dev-live.sh` when live Skia rendering is
needed; tests intentionally use the software renderer.

Use `$magik-rust-lsp` for Rust and the Slint MCP for UI behavior. Visually
verify UI changes. Let pre-commit, pre-push, and CI own automated assurance.
